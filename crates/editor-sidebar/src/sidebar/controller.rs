use std::num::NonZeroUsize;

use artisan_api::{DirListing, Project, ProjectId, ThreadId, Timestamp, UpdateThread};

use editor_core::client::CallError;
use editor_core::delivery::Delivery;
use editor_core::host::ThreadPageDirection;
use editor_core::{
    CompletionKey, CompletionScope, CompletionSlot, Core, HostId, HostInstance, Screen,
};

use super::geometry::{
    HistoryPane, PageDirection, PageLoad, PageState, PaneDistances, PaneViewports, ThreadPane,
};
use super::list::{Lists, lists};
use super::picker::FolderPicker;
use super::row::rename_value;

const MIN_TEXT_QUERY: usize = 3;
const SEARCH_PAUSE: std::time::Duration = std::time::Duration::from_millis(200);

#[derive(Debug, thiserror::Error)]
pub enum ThreadActionError {
    #[error("settle thread {thread}")]
    Settle {
        thread: ThreadId,
        #[source]
        source: CallError,
    },
    #[error("mark thread {thread} unread")]
    MarkUnread {
        thread: ThreadId,
        #[source]
        source: CallError,
    },
    #[error("archive thread {thread}")]
    Archive {
        thread: ThreadId,
        #[source]
        source: CallError,
    },
    #[error("rename thread {thread}")]
    Rename {
        thread: ThreadId,
        #[source]
        source: CallError,
    },
}

impl ThreadActionError {
    fn problem(&self) -> editor_core::Problem {
        let (headline, source) = match self {
            Self::Settle { source, .. } => ("Couldn't mark this thread as read.", source),
            Self::MarkUnread { source, .. } => ("Couldn't mark this thread as unread.", source),
            Self::Archive { source, .. } => ("Couldn't archive this thread.", source),
            Self::Rename { source, .. } => ("Couldn't rename this thread.", source),
        };
        editor_core::Problem::caused_by(format!("{headline} {}", source.fault().sentence()), self)
    }
}

enum Completion {
    Action {
        key: CompletionKey,
        result: Result<(), ThreadActionError>,
    },
    Browse {
        host: HostId,
        instance: HostInstance,
        generation: u64,
        result: Result<DirListing, CallError>,
    },
    Create {
        host: HostId,
        instance: HostInstance,
        generation: u64,
        result: Result<Project, CallError>,
    },
    Search {
        key: CompletionKey,
        query: String,
        result: Result<artisan_api::TextSearchPage, CallError>,
    },
}

struct TextHits {
    query: String,
    rows: Vec<super::list::HitRow>,
    gap: Option<SearchGap>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchGap {
    Indexing,
    Incomplete,
}

enum TextResults {
    Idle,
    Searching(String),
    Found(TextHits),
    Failed {
        query: String,
        cause: editor_core::Problem,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSearchStatus<'a> {
    Idle,
    Searching,
    Failed(&'a editor_core::Problem),
}

pub struct Sidebar {
    pub host: Option<HostId>,
    host_instance: Option<HostInstance>,
    pub open: Option<ThreadId>,
    pub lists: Lists,
    pub search: std::rc::Rc<std::cell::RefCell<editor_ui::text::Editor>>,
    pub(crate) history_open: bool,
    pub history_fraction: f32,
    history_motion: Option<(std::time::Instant, f32)>,
    motion_live: bool,
    pub(crate) rail_motion: super::rail::RailMotion,
    pub settings_motion: super::settings_nav::NavMotion,
    pub threads_edges: editor_ui::scroll_edge::ScrollEdges,
    working_pulses: std::rc::Rc<
        std::cell::RefCell<std::collections::HashMap<ThreadId, editor_ui::motion::WorkingPulse>>,
    >,
    thread_viewports: PaneViewports,
    pub error: Option<editor_core::Problem>,
    pub(crate) menu: Option<ThreadMenu>,
    pub(crate) menu_generation: u64,
    pub rename: Option<ThreadId>,
    pub rename_generation: u64,
    pub(super) drag: Option<(HostId, HostInstance, ThreadId)>,
    pub(super) effects: Vec<SidebarEffect>,
    pub picker: Option<FolderPicker>,
    picker_host: Option<HostId>,
    picker_instance: Option<HostInstance>,
    picker_generation: u64,
    browse_generation: u64,
    browse_task: Option<tokio::task::JoinHandle<()>>,
    text_search: CompletionSlot,
    actions: CompletionScope,
    text_results: TextResults,
    completions: Delivery<Completion>,
    workspace_background: bool,
}

#[derive(Clone, Debug)]
pub enum SidebarEffect {
    ShowDragPreview {
        payload: editor_core::DraggedThread,
        position: aqua_core::Point,
    },
    HideDragPreview,
    Reveal(super::list::HitRow),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadMenu {
    pub(crate) thread: ThreadId,
    pub(crate) position: aqua_core::Point,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerIdentity {
    host: HostId,
    instance: HostInstance,
    generation: u64,
}

impl Sidebar {
    pub fn drain_effects(&mut self) -> Vec<SidebarEffect> {
        std::mem::take(&mut self.effects)
    }

    pub fn drag_instance(
        &self,
        core: &Core,
        payload: &editor_core::DraggedThread,
    ) -> Option<HostInstance> {
        let (host, instance, thread) = self.drag.as_ref()?;
        if host != &payload.host || thread != &payload.thread {
            return None;
        }
        core.host_if_instance(host, *instance).map(|_| *instance)
    }
    pub fn thread_visual_factory(
        &self,
        theme: &editor_ui::theme::ArtisanTheme,
        reduced_motion: bool,
    ) -> super::view::ThreadVisualFactory {
        let pulses = std::rc::Rc::clone(&self.working_pulses);
        let theme = *theme;
        let truncate: super::row::TruncatedText = std::rc::Rc::new(
            move |text, role: editor_ui::kit::TextRole, color, selector| {
                role.text(&theme, text)
                    .gpui_source_color(color)
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .style(
                        aqua_ui::style::Style::new()
                            .min_width(aqua_ui::style::Length::Pct(0.0))
                            .flex_shrink(1.0),
                    )
                    .build()
                    .debug_selector(selector)
            },
        );
        std::rc::Rc::new(move |row, image, host| {
            let id = row.id.clone();
            let pulses = std::rc::Rc::clone(&pulses);
            let pulse: super::row::WorkingPulse = std::rc::Rc::new(move |child| {
                pulses
                    .borrow_mut()
                    .entry(id.clone())
                    .or_default()
                    .wrap(child, false)
            });
            super::row::visual_factory(&theme, reduced_motion, std::rc::Rc::clone(&truncate), pulse)(
                row, image, host,
            )
        })
    }

    pub fn new(workspace_background: bool) -> Self {
        Self {
            host: None,
            host_instance: None,
            open: None,
            lists: Lists::default(),
            search: std::rc::Rc::new(std::cell::RefCell::new(editor_ui::text::Editor::new(
                "",
                editor_ui::text::Mode::SingleLine,
            ))),
            history_open: false,
            history_fraction: 0.0,
            history_motion: None,
            motion_live: false,
            rail_motion: super::rail::RailMotion::default(),
            settings_motion: super::settings_nav::NavMotion::default(),
            threads_edges: editor_ui::scroll_edge::ScrollEdges::default(),
            working_pulses: std::rc::Rc::default(),
            thread_viewports: PaneViewports::default(),
            error: None,
            menu: None,
            menu_generation: 0,
            rename: None,
            rename_generation: 0,
            drag: None,
            effects: Vec::new(),
            picker: None,
            picker_host: None,
            picker_instance: None,
            picker_generation: 0,
            browse_generation: 0,
            browse_task: None,
            text_search: CompletionSlot::default(),
            actions: CompletionScope::default(),
            text_results: TextResults::Idle,
            completions: Delivery::new(
                NonZeroUsize::new(64).expect("invariant: the completion capacity is non-zero"),
            ),
            workspace_background,
        }
    }

    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.completions.wake_mailbox()
    }

    pub fn host_data<'a>(&self, core: &'a Core) -> Option<&'a editor_core::HostData> {
        let (id, instance) = self.host.as_ref().zip(self.host_instance)?;
        core.host_if_instance(id, instance)
    }

    pub(crate) fn screen<'a>(&self, core: &'a Core) -> &'a Screen {
        if self.workspace_background {
            core.workspace_screen()
        } else {
            core.screen()
        }
    }

    pub(crate) fn visible_picker(&self, core: &Core) -> Option<&FolderPicker> {
        let (id, instance) = self.picker_host.as_ref().zip(self.picker_instance)?;
        let host = core.host_if_instance(id, instance)?;
        if core.active_host().map(editor_core::HostData::instance) != Some(host.instance()) {
            return None;
        }
        self.picker.as_ref()
    }

    pub(crate) fn picker_identity(&self, core: &Core) -> Option<PickerIdentity> {
        self.visible_picker(core)?;
        Some(PickerIdentity {
            host: self.picker_host.clone()?,
            instance: self.picker_instance?,
            generation: self.picker_generation,
        })
    }

    pub(crate) fn dismiss_picker(&mut self) {
        if self.picker.as_ref().is_some_and(|picker| picker.creating) {
            return;
        }
        self.picker = None;
        self.picker_host = None;
        self.picker_instance = None;
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.browse_generation = self.browse_generation.wrapping_add(1);
        if let Some(task) = self.browse_task.take() {
            task.abort();
        }
    }

    pub fn sync(&mut self, core: &Core, now: Timestamp) {
        let host = core.active_host().map(|host| host.id.clone());
        let instance = core.active_host().map(editor_core::HostData::instance);
        let screen = if self.workspace_background {
            core.workspace_screen()
        } else {
            core.screen()
        };
        let open = match screen {
            Screen::Thread { thread, .. } => Some(thread.clone()),
            Screen::Home
            | Screen::NewThread { .. }
            | Screen::Settings(_)
            | Screen::Marketplace
            | Screen::Usage => None,
        };
        if let Screen::Settings(page) = screen {
            let motion_now = core.clock().now();
            self.settings_motion.sync(*page, motion_now);
            self.motion_live |= self.settings_motion.animating(motion_now);
        }
        let host_changed = self.host != host || self.host_instance != instance;
        if host_changed {
            if self.drag.take().is_some() {
                self.effects.push(SidebarEffect::HideDragPreview);
            }
            self.working_pulses.borrow_mut().clear();
            self.menu = None;
            self.rename = None;
            self.error = None;
            self.thread_viewports = PaneViewports::default();
            self.text_search.cancel();
            self.text_results = TextResults::Idle;
            self.actions.renew();
        }
        let picker_alive = self
            .picker_host
            .as_ref()
            .zip(self.picker_instance)
            .is_some_and(|(id, instance)| core.host_if_instance(id, instance).is_some());
        if self
            .picker
            .as_ref()
            .is_some_and(|picker| !picker_alive || (host_changed && !picker.creating))
        {
            self.picker = None;
            self.picker_host = None;
            self.picker_instance = None;
            self.picker_generation = self.picker_generation.wrapping_add(1);
            self.browse_generation = self.browse_generation.wrapping_add(1);
            if let Some(task) = self.browse_task.take() {
                task.abort();
            }
        }
        self.host = host;
        self.host_instance = instance;
        self.open = open;
        if host_changed {
            self.search_text(core);
        }
        self.lists = self
            .host
            .as_ref()
            .and_then(|id| core.host(id))
            .map(|host| {
                let query = self.search.borrow().text().to_owned();
                let mut lists = lists(host, self.open.as_ref(), now).searched(&query);
                lists.hits = match &self.text_results {
                    TextResults::Found(hits) if hits.query == query.trim() => hits.rows.clone(),
                    TextResults::Found(_)
                    | TextResults::Idle
                    | TextResults::Searching(_)
                    | TextResults::Failed { .. } => Vec::new(),
                };
                lists
            })
            .unwrap_or_default();
        let working = self.lists.working();
        self.working_pulses
            .borrow_mut()
            .retain(|id, _| working.contains(id));
    }

    fn finish_text_search(
        &mut self,
        core: &Core,
        now: Timestamp,
        host: &HostId,
        query: String,
        result: Result<artisan_api::TextSearchPage, CallError>,
    ) {
        match result {
            Ok(page) => {
                self.text_results = TextResults::Found(TextHits {
                    query,
                    rows: page
                        .hits
                        .iter()
                        .filter_map(super::list::HitRow::of)
                        .collect(),
                    gap: match page.coverage {
                        artisan_api::SearchCoverage::Complete => None,
                        artisan_api::SearchCoverage::Indexing => Some(SearchGap::Indexing),
                        artisan_api::SearchCoverage::Incomplete => Some(SearchGap::Incomplete),
                    },
                });
                self.sync(core, now);
            }
            Err(error) => {
                tracing::warn!(
                    %host,
                    query_chars = query.chars().count(),
                    error = %diagnostics::chain(&error),
                    "couldn't search message text"
                );
                self.text_results = TextResults::Failed {
                    query,
                    cause: editor_core::Problem::calling_forge("Couldn't search messages.", &error),
                };
            }
        }
    }

    pub fn drain(&mut self, core: &mut Core, now: Timestamp) -> bool {
        let before = super::view::visible_state(self, core);
        self.sync(core, now);
        self.page_at_viewport(core, false);
        let mut accepted = false;
        for completion in self.completions.drain() {
            match completion {
                Completion::Action {
                    key,
                    result: Err(error),
                } if self.actions.accepts(core, &key) => {
                    accepted = true;
                    let host = key.host();
                    tracing::error!(%host, error = %diagnostics::chain(&error), "a thread action failed");
                    self.error = Some(error.problem());
                }
                Completion::Browse {
                    host,
                    instance,
                    generation,
                    result,
                } if generation == self.browse_generation
                    && self.picker_host.as_ref() == Some(&host)
                    && self.picker_instance == Some(instance)
                    && core.host_if_instance(&host, instance).is_some() =>
                {
                    self.browse_task = None;
                    if let Some(picker) = &mut self.picker {
                        accepted = true;
                        picker.finish_browse(result.map_err(|error| {
                            editor_core::Problem::calling_forge(
                                "Couldn't open this folder.",
                                &error,
                            )
                        }));
                    }
                }
                Completion::Search { key, query, result }
                    if self.text_search.finish(core, &key) =>
                {
                    accepted = true;
                    self.finish_text_search(core, now, key.host(), query, result);
                }
                Completion::Create {
                    host,
                    instance,
                    generation,
                    result,
                } if generation == self.picker_generation
                    && self.picker_host.as_ref() == Some(&host)
                    && self.picker_instance == Some(instance)
                    && core.host_if_instance(&host, instance).is_some() =>
                {
                    if let Some(picker) = &mut self.picker {
                        accepted = true;
                        match result {
                            Ok(project) => {
                                self.picker = None;
                                if self.host.as_ref() == Some(&host)
                                    && self.host_instance == Some(instance)
                                    && core
                                        .active_host()
                                        .is_some_and(|current| current.instance() == instance)
                                {
                                    core.navigate(Screen::NewThread {
                                        host,
                                        project: Some(project.id),
                                    });
                                }
                            }
                            Err(error) => {
                                picker.finish_create(Some(editor_core::Problem::calling_forge(
                                    "Couldn't add this folder.",
                                    &error,
                                )));
                            }
                        }
                    }
                }
                Completion::Action { .. }
                | Completion::Browse { .. }
                | Completion::Search { .. }
                | Completion::Create { .. } => {}
            }
        }
        if accepted {
            self.sync(core, now);
        }
        before != super::view::visible_state(self, core)
    }

    pub fn open_thread(&self, core: &mut Core, thread: ThreadId) {
        if let Some((host, instance)) = self.host.as_ref().zip(self.host_instance)
            && core.host_if_instance(host, instance).is_some()
        {
            core.navigate(Screen::Thread {
                host: host.clone(),
                thread,
            });
        }
    }

    pub fn new_thread(&self, core: &mut Core, project: Option<ProjectId>) {
        let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_if_instance(id, instance))
        else {
            return;
        };
        let project = project.or_else(|| {
            self.open
                .as_ref()
                .and_then(|id| host.threads.get(id))
                .map(|thread| thread.project_id.clone())
        });
        let host = host.id.clone();
        core.navigate(Screen::NewThread { host, project });
    }

    fn call<F, Fut>(&self, core: &Core, operation: F)
    where
        F: FnOnce(editor_core::ForgeClient) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), ThreadActionError>> + Send + 'static,
    {
        let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_if_instance(id, instance))
        else {
            return;
        };
        let key = self.actions.key(&host.id, host.instance());
        let client = host.client.clone();
        let sender = self.completions.sender();
        editor_core::runtime::runtime().spawn(async move {
            let result = operation(client).await;
            sender.offer(Completion::Action { key, result }).await;
        });
    }

    pub fn settle(&self, core: &Core, thread: ThreadId) {
        let Some(activity) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_if_instance(id, instance))
            .and_then(|host| host.threads.get(&thread))
            .map(|thread| thread.activity_seq)
        else {
            return;
        };
        self.call(core, move |client| async move {
            client
                .settle_thread(&thread, activity)
                .await
                .map(drop)
                .map_err(|source| ThreadActionError::Settle { thread, source })
        });
    }

    pub(crate) fn mark_unread(&self, core: &Core, thread: ThreadId) {
        self.call(core, move |client| async move {
            client
                .mark_unread(&thread)
                .await
                .map(drop)
                .map_err(|source| ThreadActionError::MarkUnread { thread, source })
        });
    }

    pub(crate) fn archive(&self, core: &Core, thread: ThreadId) {
        self.call(core, move |client| async move {
            client
                .update_thread(
                    &thread,
                    &UpdateThread {
                        archived: Some(true),
                        ..UpdateThread::default()
                    },
                )
                .await
                .map(drop)
                .map_err(|source| ThreadActionError::Archive { thread, source })
        });
    }

    pub(crate) fn submit_rename(&mut self, core: &Core, text: &str) {
        let Some(thread) = self.rename.take() else {
            return;
        };
        let Some(title) = rename_value(text) else {
            return;
        };
        self.call(core, move |client| async move {
            client
                .update_thread(
                    &thread,
                    &UpdateThread {
                        title: Some(title),
                        ..UpdateThread::default()
                    },
                )
                .await
                .map(drop)
                .map_err(|source| ThreadActionError::Rename { thread, source })
        });
    }

    fn slide_threads(&self, core: &mut Core, direction: PageDirection) {
        let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_mut_if_instance(id, instance))
        else {
            return;
        };
        host.load_threads(
            false,
            match direction {
                PageDirection::Older => ThreadPageDirection::Older,
                PageDirection::Newer => ThreadPageDirection::Newer,
            },
        );
    }

    pub(crate) fn report_thread_viewport(
        &mut self,
        core: &mut Core,
        host_id: &HostId,
        instance: HostInstance,
        pane: ThreadPane,
        viewport: aqua_ui::collection_runtime::CollectionViewport,
        visible_projects: &[ProjectId],
    ) {
        if self.host.as_ref() != Some(host_id)
            || self.host_instance != Some(instance)
            || core.host_if_instance(host_id, instance).is_none()
        {
            return;
        }
        self.thread_viewports.report(
            pane,
            PaneDistances {
                viewport_height: viewport.viewport_height,
                above: viewport.distance_from_start,
                below: viewport.distance_from_end,
            },
        );
        self.load_visible_projects(core, visible_projects);
        self.page_at_viewport(core, false);
    }

    pub fn page_at_viewport(&self, core: &mut Core, scrolled: bool) {
        let Some(host) = self.host_data(core) else {
            return;
        };
        let page = host.thread_page(false);
        let load = match (page.is_loading() || !page.is_loaded(), page.error()) {
            (true, _) => PageLoad::Pending,
            (false, Some(_)) => PageLoad::Failed { scrolled },
            (false, None) => PageLoad::Ready,
        };
        let history = match (self.lists.history.is_empty(), self.history_open) {
            (true, _) => HistoryPane::Empty,
            (false, true) => HistoryPane::Open,
            (false, false) => HistoryPane::Closed,
        };
        let direction = self.thread_viewports.direction(PageState {
            load,
            at_latest: page.at_latest(),
            has_older: page.has_older(),
            history,
        });
        if let Some(direction) = direction {
            self.slide_threads(core, direction);
        }
    }

    pub fn toggle_history(&mut self, core: &mut Core) {
        self.history_motion = Some((core.clock().now(), self.history_fraction));
        self.history_open = !self.history_open;
        if !self.history_open {
            self.thread_viewports.forget(ThreadPane::History);
        }
        if !self.history_open
            && let Some(host) = self
                .host
                .as_ref()
                .zip(self.host_instance)
                .and_then(|(id, instance)| core.host_mut_if_instance(id, instance))
            && !host.thread_page(false).at_latest()
        {
            host.load_threads(false, ThreadPageDirection::Latest);
        }
    }

    pub fn history_visible(&self) -> bool {
        self.history_open || self.history_motion.is_some()
    }

    fn advance_history(&mut self, now: std::time::Instant, reduced_motion: bool) -> bool {
        let Some((started, from)) = self.history_motion else {
            return false;
        };
        let (fraction, finished) = super::geometry::history_fraction(
            self.history_open,
            from,
            now.saturating_duration_since(started).as_secs_f64(),
            reduced_motion,
        );
        self.history_fraction = fraction;
        if finished {
            self.history_motion = None;
        }
        !finished
    }

    pub fn advance_motion(&mut self, now: std::time::Instant, reduced_motion: bool) -> bool {
        let history = self.history_motion.is_some();
        self.advance_history(now, reduced_motion);
        let live =
            history || self.rail_motion.animating(now) || self.settings_motion.animating(now);
        std::mem::replace(&mut self.motion_live, live) || live
    }

    pub fn next_tick(&self) -> Option<Timestamp> {
        self.lists.next_tick
    }

    fn load_visible_projects(&self, core: &mut Core, projects: &[ProjectId]) {
        let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_mut_if_instance(id, instance))
        else {
            return;
        };
        for project in projects {
            host.load_repository(project, false);
            host.load_project_icon(project);
        }
    }

    pub(crate) fn retry_project_icons(&self, core: &mut Core) {
        if let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_mut_if_instance(id, instance))
        {
            host.retry_project_icons();
        }
    }

    pub(crate) fn dismiss_rollback_notice(&self, core: &mut Core) {
        if let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_mut_if_instance(id, instance))
        {
            host.dismiss_rollback_notice();
        }
    }

    pub(crate) fn retry_threads(&self, core: &mut Core) {
        if let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_mut_if_instance(id, instance))
        {
            let page = host.thread_page(false);
            let direction = if page.is_loaded() && page.has_older() {
                ThreadPageDirection::Older
            } else {
                ThreadPageDirection::Latest
            };
            host.load_threads(false, direction);
        }
    }

    pub fn add_project(&mut self, core: &Core) {
        if self.picker.as_ref().is_some_and(|picker| picker.creating) {
            return;
        }
        let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_if_instance(id, instance))
        else {
            return;
        };
        self.picker_generation = self.picker_generation.wrapping_add(1);
        self.picker_host = Some(host.id.clone());
        self.picker_instance = Some(host.instance());
        self.picker = Some(FolderPicker::default());
        self.browse(core, None);
    }

    pub(crate) fn browse(&mut self, core: &Core, path: Option<String>) {
        let Some(picker) = &mut self.picker else {
            return;
        };
        if !picker.begin_browse() {
            return;
        }
        let Some(host) = self
            .picker_host
            .as_ref()
            .zip(self.picker_instance)
            .and_then(|(id, instance)| core.host_if_instance(id, instance))
        else {
            picker.loading = false;
            return;
        };
        if let Some(task) = self.browse_task.take() {
            task.abort();
        }
        self.browse_generation = self.browse_generation.wrapping_add(1);
        let generation = self.browse_generation;
        let host_id = host.id.clone();
        let instance = host.instance();
        let client = host.client.clone();
        let sender = self.completions.sender();
        self.browse_task = Some(editor_core::runtime::runtime().spawn(async move {
            let result = client.list_dir(path.as_deref()).await;
            sender
                .offer(Completion::Browse {
                    host: host_id,
                    instance,
                    generation,
                    result,
                })
                .await;
        }));
    }

    pub fn search_text(&mut self, core: &Core) {
        self.text_search.cancel();
        let query = self.search.borrow().text().trim().to_owned();
        if query.chars().count() < MIN_TEXT_QUERY {
            self.text_results = TextResults::Idle;
            return;
        }
        let Some(host) = self.host_data(core) else {
            return;
        };
        self.text_results = TextResults::Searching(query.clone());
        let client = host.client.clone();
        let sender = self.completions.sender();
        self.text_search
            .spawn(&host.id, host.instance(), move |key| async move {
                tokio::time::sleep(SEARCH_PAUSE).await;
                let result = client
                    .text_search(&artisan_api::TextSearchQuery {
                        query: query.clone(),
                        limit: None,
                    })
                    .await;
                sender
                    .offer(Completion::Search { key, query, result })
                    .await;
            });
    }

    pub fn text_search_status(&self) -> TextSearchStatus<'_> {
        let query = self.search.borrow().text().trim().to_owned();
        match &self.text_results {
            TextResults::Searching(searched) if *searched == query => TextSearchStatus::Searching,
            TextResults::Failed {
                query: searched,
                cause,
            } if *searched == query => TextSearchStatus::Failed(cause),
            TextResults::Idle
            | TextResults::Searching(_)
            | TextResults::Found(_)
            | TextResults::Failed { .. } => TextSearchStatus::Idle,
        }
    }

    pub fn text_search_gap(&self) -> Option<SearchGap> {
        let query = self.search.borrow().text().trim().to_owned();
        match &self.text_results {
            TextResults::Found(found) if found.query == query => found.gap,
            TextResults::Idle
            | TextResults::Searching(_)
            | TextResults::Found(_)
            | TextResults::Failed { .. } => None,
        }
    }

    pub(crate) fn clear_search(&mut self, core: &Core, now: Timestamp) {
        self.search.borrow_mut().set_text("");
        self.search_text(core);
        self.sync(core, now);
    }

    pub(crate) fn create_project(&mut self, core: &Core) {
        let Some(host) = self
            .picker_host
            .as_ref()
            .zip(self.picker_instance)
            .and_then(|(id, instance)| core.host_if_instance(id, instance))
        else {
            return;
        };
        let Some(request) = self.picker.as_mut().and_then(FolderPicker::begin_create) else {
            return;
        };
        let host_id = host.id.clone();
        let instance = host.instance();
        let client = host.client.clone();
        let sender = self.completions.sender();
        let generation = self.picker_generation;
        editor_core::runtime::runtime().spawn(async move {
            let result = client.create_project(&request).await;
            sender
                .offer(Completion::Create {
                    host: host_id,
                    instance,
                    generation,
                    result,
                })
                .await;
        });
    }
}

impl std::fmt::Debug for Sidebar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sidebar")
            .field("host", &self.host)
            .field("open", &self.open)
            .finish_non_exhaustive()
    }
}
