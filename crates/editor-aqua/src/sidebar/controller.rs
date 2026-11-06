use std::num::NonZeroUsize;

use artisan_api::{DirListing, Project, ProjectId, ThreadId, Timestamp, UpdateThread};

use crate::core::delivery::Delivery;
use crate::core::host::ThreadPageDirection;
use crate::core::{Core, HostId, HostInstance, Screen};

use super::geometry::{PageDirection, PagePosition, page_direction};
use super::list::{Lists, lists};
use super::picker::FolderPicker;
use super::row::rename_value;

enum Completion {
    Action {
        host: HostId,
        instance: HostInstance,
        what: String,
        result: anyhow::Result<()>,
    },
    Browse {
        host: HostId,
        instance: HostInstance,
        generation: u64,
        result: anyhow::Result<DirListing>,
    },
    Create {
        host: HostId,
        instance: HostInstance,
        generation: u64,
        result: anyhow::Result<Project>,
    },
}

pub struct Sidebar {
    pub host: Option<HostId>,
    host_instance: Option<HostInstance>,
    pub open: Option<ThreadId>,
    last_screen: Option<Screen>,
    pub lists: Lists,
    pub history_open: bool,
    pub history_fraction: f32,
    history_motion: Option<(std::time::Instant, f32)>,
    motion_live: bool,
    pub rail_motion: super::rail::RailMotion,
    pub settings_motion: super::settings_nav::NavMotion,
    pub threads_hover: crate::ui::picker_motion::HoverSurface,
    pub profile_hover: crate::ui::picker_motion::HoverSurface,
    pub machines_hover: crate::ui::picker_motion::HoverSurface,
    pub threads_edges: crate::ui::scroll_edge::ScrollEdges,
    working_pulses: std::rc::Rc<
        std::cell::RefCell<std::collections::HashMap<ThreadId, crate::ui::motion::WorkingPulse>>,
    >,
    pub profile_open: bool,
    pub profile_generation: u64,
    pub profile_machines_open: bool,
    pub machines_generation: u64,
    pub usage_hover: Option<(artisan_api::EngineId, usize)>,
    pub thread_viewport: Option<aqua_ui::collection_runtime::CollectionViewport>,
    pub error: Option<String>,
    pub menu: Option<ThreadMenu>,
    pub menu_generation: u64,
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
    completions: Delivery<Completion>,
    workspace_background: bool,
}

pub use crate::ui::picker_motion::HoverFrame;

#[derive(Clone, Debug)]
pub enum SidebarEffect {
    ShowDragPreview {
        payload: crate::core::DraggedThread,
        position: aqua_core::Point,
    },
    HideDragPreview,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThreadMenu {
    pub thread: ThreadId,
    pub position: aqua_core::Point,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickerIdentity {
    pub host: HostId,
    pub instance: HostInstance,
    pub generation: u64,
}

impl Sidebar {
    pub fn drain_effects(&mut self) -> Vec<SidebarEffect> {
        std::mem::take(&mut self.effects)
    }

    pub fn drag_instance(
        &self,
        core: &Core,
        payload: &crate::core::DraggedThread,
    ) -> Option<HostInstance> {
        let (host, instance, thread) = self.drag.as_ref()?;
        if host != &payload.host || thread != &payload.thread {
            return None;
        }
        core.host_if_instance(host, *instance).map(|_| *instance)
    }
    pub fn thread_visual_factory(
        &self,
        theme: crate::ui::theme::ArtisanTheme,
        reduced_motion: bool,
    ) -> super::view::ThreadVisualFactory {
        let pulses = self.working_pulses.clone();
        let truncate: super::row::TruncatedText =
            std::rc::Rc::new(|text, size, color, selector| {
                aqua_ui::element::Text::new(text)
                    .font_size(size)
                    .gpui_source_color(color)
                    .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                    .style(
                        aqua_ui::style::Style::new()
                            .min_width(aqua_ui::style::Length::Px(0.0))
                            .flex_shrink(1.0),
                    )
                    .build()
                    .debug_selector(selector)
            });
        std::rc::Rc::new(move |row, image, host| {
            let id = row.id.clone();
            let pulses = pulses.clone();
            let pulse: super::row::WorkingPulse = std::rc::Rc::new(move |child, _, _, _| {
                pulses
                    .borrow_mut()
                    .entry(id.clone())
                    .or_default()
                    .wrap(child, false)
            });
            super::row::visual_factory(theme, reduced_motion, truncate.clone(), pulse)(
                row, image, host,
            )
        })
    }

    pub fn advance_threads_hover(
        &mut self,
        app: &mut aqua::UiApp,
        surface: aqua_core::NodeId,
        theme: &crate::ui::theme::ArtisanTheme,
        now: f64,
        reduced_motion: bool,
    ) -> Result<HoverFrame, String> {
        self.threads_hover
            .advance_frame(app, surface, theme, true, now, reduced_motion)
    }

    #[cfg(feature = "test-support")]
    pub async fn test_action_failure(&self, host: HostId, instance: HostInstance) {
        let _ = self
            .completions
            .sender()
            .send(Completion::Action {
                host,
                instance,
                what: "rename thread".to_owned(),
                result: Err(anyhow::anyhow!("old failure")),
            })
            .await;
    }

    #[cfg(feature = "test-support")]
    pub fn test_picker_generation(&self) -> u64 {
        self.picker_generation
    }

    #[cfg(feature = "test-support")]
    pub fn test_browse_generation(&self) -> u64 {
        self.browse_generation
    }

    #[cfg(feature = "test-support")]
    pub async fn test_browse_completion(
        &self,
        host: HostId,
        instance: HostInstance,
        generation: u64,
        listing: DirListing,
    ) {
        let _ = self
            .completions
            .sender()
            .send(Completion::Browse {
                host,
                instance,
                generation,
                result: Ok(listing),
            })
            .await;
    }

    #[cfg(feature = "test-support")]
    pub async fn test_create_completion(
        &self,
        host: HostId,
        instance: HostInstance,
        generation: u64,
        project: Project,
    ) {
        let _ = self
            .completions
            .sender()
            .send(Completion::Create {
                host,
                instance,
                generation,
                result: Ok(project),
            })
            .await;
    }

    pub fn new(workspace_background: bool) -> Self {
        Self {
            host: None,
            host_instance: None,
            open: None,
            last_screen: None,
            lists: Lists::default(),
            history_open: false,
            history_fraction: 0.0,
            history_motion: None,
            motion_live: false,
            rail_motion: super::rail::RailMotion::default(),
            settings_motion: super::settings_nav::NavMotion::default(),
            threads_hover: crate::ui::picker_motion::HoverSurface::new("sidebar-threads-hover"),
            profile_hover: crate::ui::picker_motion::HoverSurface::new("profile-hover"),
            machines_hover: crate::ui::picker_motion::HoverSurface::new("machines-hover"),
            threads_edges: crate::ui::scroll_edge::ScrollEdges::default(),
            working_pulses: std::rc::Rc::default(),
            profile_open: false,
            profile_generation: 0,
            profile_machines_open: false,
            machines_generation: 0,
            usage_hover: None,
            thread_viewport: None,
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
            completions: Delivery::new(NonZeroUsize::new(64).unwrap()),
            workspace_background,
        }
    }

    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.completions.wake_mailbox()
    }

    pub fn host_data<'a>(&self, core: &'a Core) -> Option<&'a crate::core::HostData> {
        let (id, instance) = self.host.as_ref().zip(self.host_instance)?;
        core.host_if_instance(id, instance)
    }

    pub fn screen<'a>(&self, core: &'a Core) -> &'a Screen {
        if self.workspace_background {
            core.workspace_screen()
        } else {
            core.screen()
        }
    }

    pub fn visible_picker(&self, core: &Core) -> Option<&FolderPicker> {
        let (id, instance) = self.picker_host.as_ref().zip(self.picker_instance)?;
        let host = core.host_if_instance(id, instance)?;
        if core.active_host().map(|active| active.instance()) != Some(host.instance()) {
            return None;
        }
        self.picker.as_ref()
    }

    pub fn picker_identity(&self, core: &Core) -> Option<PickerIdentity> {
        self.visible_picker(core)?;
        Some(PickerIdentity {
            host: self.picker_host.clone()?,
            instance: self.picker_instance?,
            generation: self.picker_generation,
        })
    }

    pub fn dismiss_picker(&mut self) {
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
        let instance = core.active_host().map(|host| host.instance());
        let screen = if self.workspace_background {
            core.workspace_screen()
        } else {
            core.screen()
        };
        let screen_changed = self.last_screen.as_ref() != Some(screen);
        self.last_screen = Some(screen.clone());
        let open = match screen {
            Screen::Thread { thread, .. } => Some(thread.clone()),
            _ => None,
        };
        if let Screen::Settings(page) = screen {
            let motion_now = std::time::Instant::now();
            self.settings_motion.sync(*page, motion_now);
            self.motion_live |= self.settings_motion.animating(motion_now);
        }
        let host_changed = self.host != host || self.host_instance != instance;
        if host_changed {
            if self.drag.take().is_some() {
                self.effects.push(SidebarEffect::HideDragPreview);
            }
            self.threads_hover.clear();
            self.working_pulses.borrow_mut().clear();
            self.menu = None;
            self.rename = None;
            self.error = None;
            self.profile_open = false;
            self.profile_generation = self.profile_generation.wrapping_add(1);
            self.profile_machines_open = false;
            self.usage_hover = None;
            self.thread_viewport = None;
        }
        if screen_changed || self.open != open {
            self.profile_open = false;
            self.profile_machines_open = false;
            self.usage_hover = None;
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
        self.lists = self
            .host
            .as_ref()
            .and_then(|id| core.host(id))
            .map(|host| lists(host, self.open.as_ref(), now))
            .unwrap_or_default();
        self.working_pulses.borrow_mut().retain(|id, _| {
            self.lists
                .inbox
                .iter()
                .chain(
                    self.lists
                        .history
                        .iter()
                        .flat_map(|group| group.rows.iter()),
                )
                .any(|row| &row.id == id && row.state == super::list::RowState::Working)
        });
    }

    pub fn drain(&mut self, core: &mut Core, now: Timestamp) -> bool {
        let before = super::view::visible_state(self, core);
        self.sync(core, now);
        self.page_at_viewport(core, false);
        let mut accepted = false;
        for completion in self.completions.drain() {
            match completion {
                Completion::Action {
                    host,
                    instance,
                    what,
                    result: Err(error),
                } if self.host.as_ref() == Some(&host)
                    && self.host_instance == Some(instance)
                    && core.host_if_instance(&host, instance).is_some() =>
                {
                    accepted = true;
                    tracing::error!("couldn't {what}: {error:#}");
                    self.error = Some(format!("Couldn't {what}: {error:#}"));
                }
                Completion::Action { .. } => {}
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
                        picker.finish_browse(result.map_err(|error| format!("{error:#}")));
                    }
                }
                Completion::Browse { .. } => {}
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
                            Err(error) => picker.finish_create(Some(format!("{error:#}"))),
                        }
                    }
                }
                Completion::Create { .. } => {}
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

    fn call<F, Fut>(&self, core: &Core, what: String, operation: F)
    where
        F: FnOnce(crate::core::ForgeClient) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_if_instance(id, instance))
        else {
            return;
        };
        let id = host.id.clone();
        let instance = host.instance();
        let client = host.client.clone();
        let sender = self.completions.sender();
        crate::core::runtime::runtime().spawn(async move {
            let result = operation(client).await;
            let _ = sender
                .send(Completion::Action {
                    host: id,
                    instance,
                    what,
                    result,
                })
                .await;
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
        let what = format!("settle thread {thread}");
        self.call(core, what, move |client| async move {
            client.settle_thread(&thread, activity).await.map(drop)
        });
    }

    pub fn mark_unread(&self, core: &Core, thread: ThreadId) {
        let what = format!("mark thread {thread} unread");
        self.call(core, what, move |client| async move {
            client.mark_unread(&thread).await.map(drop)
        });
    }

    pub fn archive(&self, core: &Core, thread: ThreadId) {
        let what = format!("archive thread {thread}");
        self.call(core, what, move |client| async move {
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
        });
    }

    pub fn submit_rename(&mut self, core: &Core, text: &str) {
        let Some(thread) = self.rename.take() else {
            return;
        };
        let Some(title) = rename_value(text) else {
            return;
        };
        let what = format!("rename thread {thread}");
        self.call(core, what, move |client| async move {
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
        });
    }

    pub fn slide_threads(&self, core: &mut Core, position: &PagePosition) {
        let Some(direction) = page_direction(position) else {
            return;
        };
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

    pub fn report_thread_viewport(
        &mut self,
        core: &mut Core,
        host_id: &HostId,
        instance: HostInstance,
        viewport: aqua_ui::collection_runtime::CollectionViewport,
        visible_projects: &[ProjectId],
    ) {
        if self.host.as_ref() != Some(host_id)
            || self.host_instance != Some(instance)
            || core.host_if_instance(host_id, instance).is_none()
        {
            return;
        }
        self.thread_viewport = Some(viewport);
        self.load_visible_projects(core, visible_projects);
        self.page_at_viewport(core, false);
    }

    pub fn page_at_viewport(&self, core: &mut Core, scrolled: bool) {
        let Some(viewport) = self.thread_viewport else {
            return;
        };
        let Some(host) = self.host_data(core) else {
            return;
        };
        let page = host.thread_page(false);
        let position = PagePosition {
            viewport_height: viewport.viewport_height,
            above: viewport.distance_from_start,
            below: viewport.distance_from_end,
            loading: page.loading,
            loaded: page.loaded,
            failed: page.error.is_some(),
            scrolled,
            at_latest: page.at_latest,
            has_older: page.next.is_some(),
            history_open: self.history_open,
            history_empty: self.lists.history.is_empty(),
        };
        self.slide_threads(core, &position);
    }

    pub fn toggle_history(&mut self, core: &mut Core) {
        self.history_motion = Some((std::time::Instant::now(), self.history_fraction));
        self.history_open = !self.history_open;
        if !self.history_open
            && let Some(host) = self
                .host
                .as_ref()
                .zip(self.host_instance)
                .and_then(|(id, instance)| core.host_mut_if_instance(id, instance))
            && !host.thread_page(false).at_latest
        {
            host.load_threads(false, ThreadPageDirection::Latest);
        }
    }

    pub fn history_visible(&self) -> bool {
        self.history_open || self.history_motion.is_some()
    }

    pub fn advance_history(&mut self, now: std::time::Instant, reduced_motion: bool) -> bool {
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

    pub fn load_visible_projects(&self, core: &mut Core, projects: &[ProjectId]) {
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

    pub fn retry_project_icons(&self, core: &mut Core) {
        if let Some(host) = self
            .host
            .as_ref()
            .zip(self.host_instance)
            .and_then(|(id, instance)| core.host_mut_if_instance(id, instance))
        {
            host.retry_project_icons();
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

    pub fn browse(&mut self, core: &Core, path: Option<String>) {
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
        self.browse_task = Some(crate::core::runtime::runtime().spawn(async move {
            let result = client.list_dir(path.as_deref()).await;
            let _ = sender
                .send(Completion::Browse {
                    host: host_id,
                    instance,
                    generation,
                    result,
                })
                .await;
        }));
    }

    pub fn create_project(&mut self, core: &Core) {
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
        crate::core::runtime::runtime().spawn(async move {
            let result = client.create_project(&request).await;
            let _ = sender
                .send(Completion::Create {
                    host: host_id,
                    instance,
                    generation,
                    result,
                })
                .await;
        });
    }
}
