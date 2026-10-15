//! The sidebar renders a bounded Forge thread projection. Only visible rows load metadata.
mod avatar;
mod list;
mod picker;
mod row;
#[cfg(test)]
mod tests;
mod usage;
mod usage_visual;
use crate::list::{Lists, RowData};
use crate::picker::{FolderPicker, PickerEvent};
use artisan_api::{BlobHash, ProjectId, Seq, ThreadId, Timestamp, UpdateThread};
use editor_core::{BlobImage, Core, CoreEvent, HostData, HostEvent, HostId, Screen};
use editor_ui::asset_seam::asset_glyph;
use editor_ui::measure::Measured;
use editor_ui::motion::MotionDuration;
use editor_ui::picker_motion::HoverSurface;
use editor_ui::picker_motion::{MenuEntrance, MenuMotion, animate_menu, smooth_out};
use editor_ui::theme::{ActiveTheme as _, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use editor_ui::{
    AssetId,
    button::{Button, ButtonVariant},
    dialog::Dialog,
    popover::Popover,
    text_editor::{TextEditor, TextEditorEvent},
    theme::ArtisanTheme,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, Focusable as _, FontWeight, Image,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render,
    ScrollWheelEvent, Stateful, StatefulInteractiveElement as _, Styled as _, Subscription, Task,
    Window, anchored, deferred, div, point, px,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};
const PROFILE_MENU_OPEN: Duration = MotionDuration::Fast.as_duration();
const PROFILE_MENU_CLOSE: Duration = MotionDuration::Quick.as_duration();
use std::{
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    time::{Duration, Instant},
};

#[derive(Clone)]
enum Entry {
    Thread(RowData),
    History,
    Heading(&'static str),
    Gap(f32),
    Spacer,
}
impl Entry {
    fn same_key(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Thread(a), Self::Thread(b)) => a.id == b.id,
            (Self::History, Self::History) | (Self::Spacer, Self::Spacer) => true,
            (Self::Heading(a), Self::Heading(b)) => a == b,
            (Self::Gap(a), Self::Gap(b)) => a == b,
            _ => false,
        }
    }
}
struct IconEntry {
    project_seq: Seq,
    hash: Option<BlobHash>,
    failed: bool,
    checked: Instant,
}
impl IconEntry {
    fn fresh(&self, now: Instant) -> bool {
        let ttl = if self.failed || self.hash.is_none() {
            Duration::from_secs(5)
        } else {
            Duration::from_secs(300)
        };
        now.saturating_duration_since(self.checked) < ttl
    }
}
struct IconRequest {
    project_seq: Seq,
    _task: Task<()>,
}
pub struct Sidebar {
    core: Entity<Core>,
    theme: ArtisanTheme,
    host: Option<Entity<HostData>>,
    open: Option<ThreadId>,
    lists: Lists,
    entries: Vec<Entry>,
    threads_list: gpui::ListState,
    threads_height: f32,
    threads_glide: editor_ui::wheel_glide::WheelGlide,
    threads_glide_scheduled: bool,
    history_open: bool,
    nav_hover: HoverSurface,
    threads_hover: HoverSurface,
    /// Cached thread rows the list laid out since the last render; the
    /// rest are dropped, so only rows on screen keep an entity.
    rows: HashMap<ThreadId, (Entity<row::ThreadRow>, Measured)>,
    laid_out: HashSet<ThreadId>,
    profile_origin: Measured,
    avatars: HashMap<String, Arc<Image>>,
    icons: HashMap<ProjectId, IconEntry>,
    icon_order: VecDeque<ProjectId>,
    icon_tasks: HashMap<ProjectId, IconRequest>,
    icon_generation: u64,
    profile: Option<Entity<usage::Account>>,
    profile_open: bool,
    profile_motion: Rc<RefCell<MenuMotion>>,
    profile_settle: Option<Task<()>>,
    chevron_toggled: Option<Instant>,
    menu: Option<Menu>,
    rename: Option<Rename>,
    picker: Option<(Entity<FolderPicker>, Subscription)>,
    picker_host: Option<HostId>,
    error: Option<String>,
    #[cfg(test)]
    call_completion_gate: Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        tokio::sync::oneshot::Receiver<bool>,
    )>,
    #[cfg(test)]
    call_completions: usize,
    tick: Option<(Timestamp, Task<()>)>,
    _core: Subscription,
    _host: Option<Subscription>,
}
struct Menu {
    target: MenuTarget,
    position: Point<Pixels>,
}
#[derive(Clone)]
enum MenuTarget {
    Thread(ThreadId),
}
struct Rename {
    editor: Entity<TextEditor>,
    _submitted: Subscription,
}
impl Sidebar {
    pub fn new(core: Entity<Core>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&core, |this, _, event: &CoreEvent, cx| {
            if matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged) {
                this.reset_profile();
                this.sync(cx);
            }
        });
        let mut this = Self {
            core,
            theme: *cx.theme(),
            host: None,
            open: None,
            lists: Lists::default(),
            entries: Vec::new(),
            threads_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(0.0)),
            threads_height: 0.0,
            threads_glide: editor_ui::wheel_glide::WheelGlide::default(),
            threads_glide_scheduled: false,
            history_open: false,
            nav_hover: HoverSurface::default(),
            threads_hover: HoverSurface::default(),
            rows: HashMap::new(),
            laid_out: HashSet::new(),
            profile_origin: Measured::default(),
            avatars: HashMap::new(),
            icons: HashMap::new(),
            icon_order: VecDeque::new(),
            icon_tasks: HashMap::new(),
            icon_generation: 0,
            profile: None,
            profile_open: false,
            profile_motion: Rc::default(),
            profile_settle: None,
            chevron_toggled: None,
            menu: None,
            rename: None,
            picker: None,
            picker_host: None,
            error: None,
            #[cfg(test)]
            call_completion_gate: None,
            #[cfg(test)]
            call_completions: 0,
            tick: None,
            _core: subscription,
            _host: None,
        };
        this.sync(cx);
        this
    }
    pub fn core(&self) -> &Entity<Core> {
        &self.core
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let host = core.active_host().cloned();
        let open = match core.screen() {
            Screen::Thread { thread, .. } => Some(thread.clone()),
            _ => None,
        };
        if host.as_ref().map(Entity::entity_id) != self.host.as_ref().map(Entity::entity_id) {
            self._host = host.as_ref().map(|host| {
                cx.subscribe(host, |this, _, event: &HostEvent, cx| match event {
                    HostEvent::Changed | HostEvent::ThreadChanged { .. } => this.refresh(cx),
                    HostEvent::Reset => {
                        this.clear_icons();
                        this.refresh(cx);
                    }
                    HostEvent::ImagesChanged | HostEvent::EngineInstall { .. } => cx.notify(),
                    _ => {}
                })
            });
            self.host = host;
            self.clear_icons();
            self.menu = None;
            self.rename = None;
            self.reset_profile();
            self.error = None;
            // A creation already accepted by Forge must retain its acknowledgement task.
            if self
                .picker
                .as_ref()
                .is_some_and(|(picker, _)| !picker.read(cx).creating())
            {
                self.picker = None;
            }
        }
        if self.open != open {
            self.reset_profile();
        }
        self.open = open;
        self.refresh(cx);
    }
    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.reconcile_icons(cx);
        let now = artisan_api::now();
        self.lists = self
            .host
            .as_ref()
            .map(|host| list::lists(host.read(cx), self.open.as_ref(), now))
            .unwrap_or_default();
        self.rebuild_entries();
        self.schedule_tick(now, cx);
        cx.notify();
    }
    fn rebuild_entries(&mut self) {
        let previous = std::mem::take(&mut self.entries);
        for (index, row) in self.lists.inbox.iter().enumerate() {
            if index > 0 {
                self.entries.push(Entry::Gap(2.0));
            }
            self.entries.push(Entry::Thread(row.clone()));
        }
        if !self.lists.history.is_empty() {
            self.entries.push(Entry::Spacer);
            self.entries.push(Entry::History);
            if self.history_open {
                self.entries.push(Entry::Gap(2.0));
                for (index, group) in self.lists.history.iter().enumerate() {
                    if index > 0 {
                        self.entries.push(Entry::Gap(12.0));
                    }
                    if let Some(label) = group.age.label() {
                        self.entries.push(Entry::Heading(label));
                    }
                    for (index, row) in group.rows.iter().enumerate() {
                        if index > 0 {
                            self.entries.push(Entry::Gap(2.0));
                        }
                        self.entries.push(Entry::Thread(row.clone()));
                    }
                }
            }
        }
        let prefix = previous
            .iter()
            .zip(&self.entries)
            .take_while(|(a, b)| a.same_key(b))
            .count();
        let suffix = previous[prefix..]
            .iter()
            .rev()
            .zip(self.entries[prefix..].iter().rev())
            .take_while(|(a, b)| a.same_key(b))
            .count();
        let removed = prefix..previous.len() - suffix;
        let inserted = self.entries.len() - prefix - suffix;
        if !removed.is_empty() || inserted > 0 {
            self.threads_list.splice(removed, inserted);
            // The spacer depends on all registered row heights, even when its
            // own key survives the splice.
            if let Some(index) = self
                .entries
                .iter()
                .position(|entry| matches!(entry, Entry::Spacer))
            {
                self.threads_list.remeasure_items(index..index + 1);
            }
        }
    }
    fn entry_height(&self, entry: &Entry) -> f32 {
        match entry {
            Entry::Thread(_) => row::ROW_HEIGHT,
            Entry::History => 34.0,
            Entry::Heading(_) => 21.0,
            Entry::Gap(height) => *height,
            Entry::Spacer => 0.0,
        }
    }
    fn history_spacer(&self) -> f32 {
        (self.threads_height
            - self
                .entries
                .iter()
                .map(|entry| self.entry_height(entry))
                .sum::<f32>())
        .max(12.0)
    }
    fn threads_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = self.threads_list.clone();
        let max = f32::from(handle.max_offset_for_scrollbar().y).abs();
        if max <= 0.0 {
            return;
        }
        cx.stop_propagation();
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        let offset = handle.scroll_px_offset_for_scrollbar();
        let current = f32::from(offset.y);
        if event.delta.precise() || cx.reduce_motion() {
            self.threads_glide.cancel();
            handle
                .set_offset_from_scrollbar(point(offset.x, px((current + delta).clamp(-max, 0.0))));
            cx.notify();
            return;
        }
        self.threads_glide.push(current, delta, max);
        self.schedule_threads_glide(window, cx);
    }
    fn schedule_threads_glide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.threads_glide.active() || self.threads_glide_scheduled {
            return;
        }
        self.threads_glide_scheduled = true;
        cx.on_next_frame(window, |this, window, cx| {
            this.threads_glide_scheduled = false;
            let handle = this.threads_list.clone();
            let offset = handle.scroll_px_offset_for_scrollbar();
            let max = f32::from(handle.max_offset_for_scrollbar().y).abs();
            if let Some(next) = this.threads_glide.step(f32::from(offset.y), max) {
                handle.set_offset_from_scrollbar(point(offset.x, px(next)));
                cx.notify();
            }
            this.schedule_threads_glide(window, cx);
        });
    }
    fn schedule_tick(&mut self, now: Timestamp, cx: &mut Context<Self>) {
        let Some(at) = self.lists.next_tick else {
            self.tick = None;
            return;
        };
        if self
            .tick
            .as_ref()
            .is_some_and(|(scheduled, _)| *scheduled == at)
        {
            return;
        }
        let task = cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(
                    u64::try_from(at - now).unwrap_or(0),
                ))
                .await;
            let _ = this.update(cx, |this, cx| {
                this.tick = None;
                this.refresh(cx);
            });
        });
        self.tick = Some((at, task));
    }
    /// Runs a Forge call; a failure is logged and shown, never silent.
    fn call<T: 'static>(
        &mut self,
        cx: &mut Context<Self>,
        what: String,
        call: impl Future<Output = anyhow::Result<T>> + 'static,
    ) {
        let origin = self.host_id(cx);
        #[cfg(test)]
        let completion_gate = self.call_completion_gate.take();
        cx.spawn(async move |sidebar, cx| {
            let result = call.await;
            #[cfg(test)]
            let result = if let Some((arrived, release)) = completion_gate {
                arrived.store(true, std::sync::atomic::Ordering::Release);
                if release.await.unwrap_or(false) {
                    Err(anyhow::anyhow!("injected sidebar acknowledgement failure"))
                } else {
                    result
                }
            } else {
                result
            };
            #[cfg(test)]
            {
                let _ = sidebar.update(cx, |sidebar, _| sidebar.call_completions += 1);
            }
            if let Err(error) = result {
                tracing::error!("couldn't {what}: {error:#}");
                sidebar
                    .update(cx, |sidebar, cx| {
                        if sidebar.host_id(cx) != origin {
                            return;
                        }
                        sidebar.error = Some(format!("Couldn't {what}: {error:#}"));
                        cx.notify();
                    })
                    .ok();
            }
        })
        .detach();
    }

    fn host_id(&self, cx: &App) -> Option<HostId> {
        self.host.as_ref().map(|host| host.read(cx).id.clone())
    }

    fn navigate(&self, screen: Screen, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| core.navigate(screen, cx));
    }

    pub(crate) fn open_thread(&mut self, thread: ThreadId, cx: &mut Context<Self>) {
        if let Some(host) = self.host_id(cx) {
            self.navigate(Screen::Thread { host, thread }, cx);
        }
    }

    fn new_thread(&mut self, project: Option<ProjectId>, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        let project = project.or_else(|| {
            let open = self.open.as_ref()?;
            host.read(cx)
                .threads
                .get(open)
                .map(|thread| thread.project_id.clone())
        });
        let host = host.read(cx).id.clone();
        self.navigate(Screen::NewThread { host, project }, cx);
    }

    pub(crate) fn open_thread_menu(
        &mut self,
        thread: ThreadId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.menu = Some(Menu {
            target: MenuTarget::Thread(thread),
            position,
        });
        cx.notify();
    }

    fn update_thread(
        &mut self,
        thread: ThreadId,
        what: &str,
        update: UpdateThread,
        cx: &mut Context<Self>,
    ) {
        let Some(host) = &self.host else {
            return;
        };
        let client = host.read(cx).client.clone();
        self.call(cx, format!("{what} thread {thread}"), async move {
            client.update_thread(&thread, &update).await
        });
    }

    fn mark_unread(&mut self, thread: ThreadId, cx: &mut Context<Self>) {
        let Some(host) = &self.host else {
            return;
        };
        let client = host.read(cx).client.clone();
        self.call(cx, format!("mark thread {thread} unread"), async move {
            client.mark_unread(&thread).await
        });
    }

    fn start_rename(&mut self, thread: ThreadId, window: &mut Window, cx: &mut Context<Self>) {
        let title = self
            .host
            .as_ref()
            .and_then(|host| {
                host.read(cx)
                    .threads
                    .get(&thread)
                    .and_then(|thread| thread.title.clone())
            })
            .unwrap_or_default();
        let editor = cx.new(|cx| {
            let mut editor = TextEditor::new(&title, "Thread title", cx).single_line();
            editor.select_whole_text(cx);
            editor
        });
        editor_ui::overlay::begin("sidebar-rename", window, cx);
        let overlay_window = window.window_handle();
        window.focus(&editor.focus_handle(cx), cx);
        let submitted = cx.subscribe(
            &editor,
            move |sidebar, editor, event: &TextEditorEvent, cx| {
                if *event != TextEditorEvent::Submitted {
                    return;
                }
                let title = editor.read(cx).text().trim().to_owned();
                sidebar.rename = None;
                let _ = overlay_window.update(cx, |_, window, cx| {
                    editor_ui::overlay::dismiss("sidebar-rename", window, cx)
                });
                if !title.is_empty() {
                    sidebar.update_thread(
                        thread.clone(),
                        "rename",
                        UpdateThread {
                            title: Some(title),
                            ..UpdateThread::default()
                        },
                        cx,
                    );
                }
                cx.notify();
            },
        );
        self.rename = Some(Rename {
            editor,
            _submitted: submitted,
        });
        cx.notify();
    }

    /// Settles a thread into History: read, and kept there until new
    /// activity. Forge holds the settled marks so every Editor agrees.
    fn settle(&mut self, thread: &ThreadId, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        let data = host.read(cx);
        let Some(activity) = data.threads.get(thread).map(|thread| thread.activity_seq) else {
            return;
        };
        let client = data.client.clone();
        let thread = thread.clone();
        self.call(cx, format!("settle thread {thread}"), async move {
            client.settle_thread(&thread, activity).await
        });
    }

    /// Every project is selected through the paired Forge.
    pub fn add_project(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        if self
            .picker
            .as_ref()
            .is_some_and(|(picker, _)| picker.read(cx).creating())
        {
            return;
        }
        let (client, host_id) = (host.read(cx).client.clone(), host.read(cx).id.clone());
        self.picker_host = Some(host_id.clone());
        let theme = self.theme;
        let picker = cx.new(|cx| FolderPicker::new(client, theme, cx));
        let subscription =
            cx.subscribe(
                &picker,
                move |sidebar, _, event: &PickerEvent, cx| match event {
                    PickerEvent::Added(project) => {
                        sidebar.picker = None;
                        if sidebar.host_id(cx).as_ref() != Some(&host_id) {
                            cx.notify();
                            return;
                        }
                        sidebar.navigate(
                            Screen::NewThread {
                                host: host_id.clone(),
                                project: Some(project.id.clone()),
                            },
                            cx,
                        );
                        cx.notify();
                    }
                },
            );
        self.picker = Some((picker, subscription));
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn inbox(&self) -> Vec<ThreadId> {
        self.lists.inbox.iter().map(|row| row.id.clone()).collect()
    }

    #[cfg(test)]
    pub(crate) fn history(&self) -> Vec<ThreadId> {
        self.lists
            .history
            .iter()
            .flat_map(|group| &group.rows)
            .map(|row| row.id.clone())
            .collect()
    }
}

impl Sidebar {
    fn clear_icons(&mut self) {
        self.icon_generation = self.icon_generation.wrapping_add(1);
        self.icons.clear();
        self.icon_order.clear();
        self.icon_tasks.clear();
    }
    fn reconcile_icons(&mut self, cx: &App) {
        let Some(host) = &self.host else {
            self.clear_icons();
            return;
        };
        let projects = &host.read(cx).projects;
        self.icons.retain(|id, entry| {
            projects
                .get(id)
                .is_some_and(|project| project.seq == entry.project_seq)
        });
        self.icon_tasks.retain(|id, request| {
            projects
                .get(id)
                .is_some_and(|project| project.seq == request.project_seq)
        });
        self.icon_order.retain(|id| self.icons.contains_key(id));
    }
    fn load_visible_projects(&mut self, projects: &[ProjectId], cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        if !host.read(cx).is_live() {
            return;
        }
        host.update(cx, |host, cx| {
            for project in projects {
                host.load_repository(project, false, cx);
            }
        });
        let now = Instant::now();
        for project in projects {
            let Some(project_seq) = host
                .read(cx)
                .projects
                .get(project)
                .map(|project| project.seq)
            else {
                continue;
            };
            if let Some(entry) = self.icons.get(project) {
                if entry.project_seq == project_seq {
                    if let Some(hash) = &entry.hash {
                        let hash = hash.clone();
                        host.update(cx, |host, cx| host.load_image(hash, false, cx));
                    }
                    self.icon_order.retain(|id| id != project);
                    self.icon_order.push_back(project.clone());
                    if entry.fresh(now) {
                        continue;
                    }
                }
            }
            if self.icon_tasks.contains_key(project) || self.icon_tasks.len() >= 4 {
                continue;
            }
            let project = project.clone();
            let key = project.clone();
            let origin = host.entity_id();
            let generation = self.icon_generation;
            let client = host.read(cx).client.clone();
            let host = host.clone();
            let task = cx.spawn(async move |this, cx| {
                let result = client.project_icon(&project).await;
                let _ = this.update(cx, |this, cx| {
                    if this.icon_generation != generation
                        || this.host.as_ref().map(Entity::entity_id) != Some(origin)
                        || host
                            .read(cx)
                            .projects
                            .get(&project)
                            .is_none_or(|current| current.seq != project_seq)
                    {
                        return;
                    }
                    this.icon_tasks.remove(&project);
                    let failed = result.is_err();
                    let hash = match result {
                        Ok(blob) => blob.map(|blob| blob.hash),
                        Err(_) => this
                            .icons
                            .get(&project)
                            .and_then(|entry| entry.hash.clone()),
                    };
                    if let Some(hash) = &hash {
                        host.update(cx, |host, cx| host.load_image(hash.clone(), false, cx));
                    }
                    this.icon_order.retain(|id| id != &project);
                    this.icon_order.push_back(project.clone());
                    this.icons.insert(
                        project,
                        IconEntry {
                            project_seq,
                            hash,
                            failed,
                            checked: Instant::now(),
                        },
                    );
                    while this.icons.len() > 128 {
                        if let Some(old) = this.icon_order.pop_front() {
                            this.icons.remove(&old);
                        } else {
                            break;
                        }
                    }
                    cx.notify();
                });
            });
            self.icon_tasks.insert(
                key,
                IconRequest {
                    project_seq,
                    _task: task,
                },
            );
        }
    }
    /// A laid-out thread row: its cached entity, fed through set-and-notify,
    /// with the probes a cached view cannot carry (its paint replays).
    fn thread_row(&mut self, inputs: row::RowInputs, cx: &mut Context<Self>) -> AnyElement {
        let id = inputs.data.id.clone();
        self.laid_out.insert(id.clone());
        let (row, bounds) = if let Some((row, bounds)) = self.rows.get(&id) {
            row.update(cx, |row, cx| row.set(inputs, cx));
            (row.clone(), bounds.clone())
        } else {
            let bounds = Measured::default();
            let (theme, sidebar, hover) = (
                self.theme,
                cx.entity().downgrade(),
                self.threads_hover.clone(),
            );
            let row_bounds = bounds.clone();
            let row = cx.new(|_| row::ThreadRow::new(inputs, theme, sidebar, hover, row_bounds));
            self.rows.insert(id.clone(), (row.clone(), bounds.clone()));
            (row, bounds)
        };
        // The row's name sits out here: a cached view's replayed paint does
        // not record debug bounds.
        let selector = format!("sidebar-thread-{id}");
        div()
            .relative()
            .w_full()
            .h(px(row::ROW_HEIGHT))
            .debug_selector(move || selector.clone())
            .child(
                row.cached(
                    gpui::StyleRefinement::default()
                        .w_full()
                        .h(px(row::ROW_HEIGHT)),
                ),
            )
            .child(bounds.record())
            .child(self.threads_hover.row_probe(format!("sidebar-thread-{id}")))
            .into_any_element()
    }

    fn render_entry(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = self.entries.get(index).cloned() else {
            return div().into_any_element();
        };
        let content = match entry {
            Entry::Thread(data) => {
                self.load_visible_projects(std::slice::from_ref(&data.project), cx);
                let image = self
                    .icons
                    .get(&data.project)
                    .and_then(|entry| entry.hash.as_ref())
                    .and_then(|hash| self.host.as_ref()?.read(cx).image(hash))
                    .and_then(|image| match image {
                        BlobImage::Ready(image) => Some(image),
                        _ => None,
                    });
                let repository = self
                    .host
                    .as_ref()
                    .and_then(|host| host.read(cx).repository(&data.project))
                    .map(|repository| repository.host.clone());
                let inputs = row::RowInputs {
                    data,
                    image,
                    host: repository,
                    reduced: cx.reduce_motion(),
                };
                self.thread_row(inputs, cx)
            }
            Entry::Gap(height) => div().h(px(height)).into_any_element(),
            Entry::Spacer => div().h(px(self.history_spacer())).into_any_element(),
            Entry::Heading(label) => div()
                .h(px(21.0))
                .flex()
                .items_center()
                .px(px(8.0))
                .text_xs()
                .text_color(self.theme.colors.muted_foreground.to_paint())
                .child(label)
                .into_any_element(),
            Entry::History => {
                let count = self.lists.history_len();
                self.nav_row(
                    "sidebar-history",
                    "history",
                    &self.threads_hover,
                    AssetId::TABLER_ARCHIVE,
                    "History",
                    false,
                    cx,
                )
                .child(div().flex_1())
                .child(editor_ui::glass::glass_badge(
                    &self.theme,
                    count.to_string(),
                ))
                .child(
                    asset_glyph(if self.history_open {
                        AssetId::TABLER_CHEVRON_DOWN
                    } else {
                        AssetId::TABLER_CHEVRON_RIGHT
                    })
                    .size(px(12.0))
                    .text_color(self.theme.colors.workspace_secondary.to_paint()),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.history_open = !this.history_open;
                    this.rebuild_entries();
                    cx.notify();
                }))
                .into_any_element()
            }
        };
        div()
            .id(gpui::SharedString::from(format!("sidebar-entry-{index}")))
            .w_full()
            .child(content)
            .on_scroll_wheel(cx.listener(Self::threads_wheel))
            .into_any_element()
    }
    fn render_menu(&self, menu: &Menu, cx: &mut Context<Self>) -> AnyElement {
        let MenuTarget::Thread(thread) = menu.target.clone();
        let theme = self.theme;
        let item = |id: &'static str, asset: AssetId, label: &'static str, destructive: bool| {
            let foreground = if destructive {
                theme.colors.destructive.to_paint()
            } else {
                theme.colors.workspace_foreground.to_paint()
            };
            div()
                .id(id)
                .role(gpui::Role::MenuItem)
                .aria_label(label)
                .focusable()
                .tab_index(0)
                .debug_selector(move || id.to_owned())
                .relative()
                .px(px(12.0))
                .py(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .rounded(RadiusTokens::value(RadiusStep::LgMid))
                .cursor_pointer()
                .hover(move |style| {
                    editor_ui::hover_card::hover_fill(
                        style,
                        &theme,
                        editor_ui::hover_card::HoverFill::Plain,
                    )
                })
                .child(
                    div().size(px(16.0)).flex_none().child(
                        asset_glyph(asset)
                            .size(px(15.0))
                            .text_color(if destructive {
                                foreground
                            } else {
                                theme.colors.workspace_secondary.to_paint()
                            }),
                    ),
                )
                .child(
                    div()
                        .text_size(TypographyTokens::size(TextStep::Sm))
                        .line_height(px(20.0))
                        .whitespace_nowrap()
                        .text_color(foreground)
                        .child(label),
                )
        };
        let mut content = div().w(px(198.0)).flex().flex_col();
        let settleable = self
            .lists
            .inbox
            .iter()
            .any(|row| row.id == thread && row.settleable);
        if settleable {
            let thread = thread.clone();
            content = content.child(
                item(
                    "sidebar-menu-settle",
                    AssetId::TABLER_CIRCLE_CHECK,
                    "Settle Thread",
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    editor_ui::overlay::dismiss("sidebar-menu", window, cx);
                    this.menu = None;
                    this.settle(&thread, cx);
                    cx.notify();
                })),
            );
        }
        let rename = thread.clone();
        content = content.child(
            item(
                "sidebar-menu-rename",
                AssetId::TABLER_PENCIL,
                "Rename Thread",
                false,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                editor_ui::overlay::dismiss("sidebar-menu", window, cx);
                this.menu = None;
                this.start_rename(rename.clone(), window, cx);
            })),
        );
        let already_unread = self
            .host
            .as_ref()
            .and_then(|host| host.read(cx).threads.get(&thread))
            .is_some_and(|thread| thread.is_unread() || thread.activity_seq == 0);
        if !already_unread {
            let unread = thread.clone();
            content = content.child(
                item(
                    "sidebar-menu-unread",
                    AssetId::TABLER_BELL,
                    "Mark as Unread",
                    false,
                )
                .on_click(cx.listener(move |this, _, window, cx| {
                    editor_ui::overlay::dismiss("sidebar-menu", window, cx);
                    this.menu = None;
                    this.mark_unread(unread.clone(), cx);
                    cx.notify();
                })),
            );
        }
        content = content.child(
            div()
                .h(px(1.0))
                .my(px(4.0))
                .bg(theme.colors.border.with_alpha(0.5).to_paint()),
        );
        content = content.child(
            item(
                "sidebar-menu-archive",
                AssetId::TABLER_ARCHIVE,
                "Archive Thread",
                true,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                editor_ui::overlay::dismiss("sidebar-menu", window, cx);
                this.menu = None;
                this.update_thread(
                    thread.clone(),
                    "archive",
                    UpdateThread {
                        archived: Some(true),
                        ..Default::default()
                    },
                    cx,
                );
                cx.notify();
            })),
        );
        let owner = cx.entity().downgrade();
        Popover::new(
            "sidebar-menu",
            "Thread actions",
            menu.position,
            content,
            move |_, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.menu = None;
                    cx.notify();
                });
            },
        )
        .menu_padding(px(4.0))
        .quiet_glass_menu()
        .into_any_element()
    }
    #[allow(clippy::too_many_arguments)]
    fn nav_row(
        &self,
        id: &'static str,
        hover_id: &'static str,
        surface: &HoverSurface,
        icon: AssetId,
        label: &'static str,
        current: bool,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let hover = surface.clone();
        div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(label)
            .aria_selected(current)
            .focusable()
            .tab_index(0)
            .debug_selector(move || id.to_owned())
            .relative()
            .w_full()
            .h(px(34.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::Sm))
            .cursor_pointer()
            .child(surface.row_probe(hover_id))
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                hover.hovered(hover_id, *hovered);
                cx.notify();
            }))
            .child(
                div()
                    .size(px(16.0))
                    .flex_none()
                    .text_color(self.theme.colors.workspace_secondary.to_paint())
                    .child(asset_glyph(icon).size(px(15.0))),
            )
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .text_color(if current {
                        self.theme.colors.banner_info.to_paint()
                    } else {
                        self.theme.colors.workspace_foreground.to_paint()
                    })
                    .child(label),
            )
    }

    fn render_profile_trigger(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (seed, name, machine, status) = match &self.host {
            Some(host) => {
                let host = host.read(cx);
                let seed = self
                    .core
                    .read(cx)
                    .hosts()
                    .find(|(config, _)| config.id == host.id)
                    .map_or_else(
                        || host.id.0.clone(),
                        |(config, _)| config.fingerprint.clone(),
                    );
                let status = match &host.status {
                    editor_core::ConnectionStatus::Live => None,
                    editor_core::ConnectionStatus::Connecting => Some("Connecting…".to_owned()),
                    editor_core::ConnectionStatus::Offline { reason, .. } => {
                        Some(format!("Offline: {reason}"))
                    }
                };
                (
                    seed,
                    host.account
                        .as_ref()
                        .map_or_else(|| host.name.clone(), |account| account.user.clone()),
                    host.name.clone(),
                    status,
                )
            }
            None => (
                "no-host".to_owned(),
                "Artisan".to_owned(),
                "No host".to_owned(),
                None,
            ),
        };
        let image = self
            .avatars
            .entry(seed.clone())
            .or_insert_with(|| avatar::avatar_image(&seed))
            .clone();
        let origin = self.profile_origin.clone();
        let hover = self.nav_hover.clone();
        let turn = self.chevron_turn(window, cx);
        div()
            .id("artisan-desktop-profile-trigger")
            .role(gpui::Role::Button)
            .aria_label("Account and usage")
            .aria_expanded(self.profile_open)
            .focusable()
            .tab_index(0)
            .debug_selector(|| "artisan-desktop-profile-trigger".into())
            .relative()
            .w_full()
            .h(px(44.0))
            .p(px(5.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(self.nav_hover.row_probe("profile"))
            .child(origin.record())
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                hover.hovered("profile", *hovered);
                cx.notify();
            }))
            .child(
                div()
                    .size(px(32.0))
                    .flex_shrink_0()
                    .rounded(RadiusTokens::value(RadiusStep::Md))
                    .overflow_hidden()
                    .child(avatar::avatar(image)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(TypographyTokens::size(TextStep::Sm))
                            .line_height(px(16.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(self.theme.colors.workspace_foreground.to_paint())
                            .child(name),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(TypographyTokens::size(TextStep::X3s))
                            .line_height(px(12.0))
                            .text_color(if status.is_some() {
                                self.theme.colors.banner_warning.to_paint()
                            } else {
                                self.theme.colors.workspace_secondary.to_paint()
                            })
                            .child(status.unwrap_or(machine)),
                    ),
            )
            .child(
                gpui::svg()
                    .path(AssetId::TABLER_CHEVRON_UP.as_str())
                    .size(px(15.0))
                    .text_color(self.theme.colors.workspace_secondary.to_paint())
                    .with_transformation(gpui::Transformation::rotate(gpui::radians(
                        turn * std::f32::consts::PI,
                    ))),
            )
            .on_click(cx.listener(|this, _, _, cx| {
                cx.stop_propagation();
                this.show_profile(cx);
            }))
            .into_any_element()
    }
    fn reset_profile(&mut self) {
        self.profile = None;
        self.profile_open = false;
        self.profile_motion.borrow_mut().hide();
        self.profile_settle = None;
        self.chevron_toggled = None;
    }
    fn show_profile(&mut self, cx: &mut Context<Self>) {
        if self.profile_open {
            self.close_profile(cx);
            return;
        }
        if self.profile.is_none() {
            let core = self.core.clone();
            let owner = cx.entity().downgrade();
            self.profile = Some(cx.new(|cx| {
                usage::Account::new(core, cx).with_close(move |cx| {
                    let _ = owner.update(cx, |this, cx| this.close_profile(cx));
                })
            }));
        }
        self.profile_open = true;
        self.chevron_toggled = Some(Instant::now());
        let generation = self.profile_motion.borrow_mut().begin_open();
        self.settle_profile(generation, true, cx);
        cx.notify();
    }
    fn close_profile(&mut self, cx: &mut Context<Self>) {
        if !self.profile_open {
            return;
        }
        self.profile_open = false;
        self.chevron_toggled = Some(Instant::now());
        if let Some(account) = &self.profile {
            account.update(cx, |account, cx| account.clear_interaction(cx));
        }
        let generation = self.profile_motion.borrow_mut().begin_close();
        self.settle_profile(generation, false, cx);
        cx.notify();
    }
    fn settle_profile(&mut self, generation: u64, open: bool, cx: &mut Context<Self>) {
        let settle = move |this: &mut Self| {
            let finished = {
                let mut motion = this.profile_motion.borrow_mut();
                if open {
                    motion.finish_open(generation)
                } else {
                    motion.finish_close(generation)
                }
            };
            if finished && !open {
                this.profile = None;
            }
            finished
        };
        if cx.reduce_motion() {
            settle(self);
            self.profile_settle = None;
            return;
        }
        let duration = if open {
            PROFILE_MENU_OPEN
        } else {
            PROFILE_MENU_CLOSE
        };
        self.profile_settle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let _ = this.update(cx, |this, cx| {
                if settle(this) {
                    this.profile_settle = None;
                    cx.notify();
                }
            });
        }));
    }
    /// The chevron's turn: flips over 250 ms on the smooth-out curve.
    fn chevron_turn(&self, window: &mut Window, cx: &App) -> f32 {
        let settled = if self.profile_open { 1.0 } else { 0.0 };
        let Some(toggled) = self.chevron_toggled.filter(|_| !cx.reduce_motion()) else {
            return settled;
        };
        let progress = toggled.elapsed().as_secs_f32() / PROFILE_MENU_OPEN.as_secs_f32();
        if progress >= 1.0 {
            return settled;
        }
        window.request_animation_frame();
        let eased = smooth_out(progress);
        if self.profile_open {
            eased
        } else {
            1.0 - eased
        }
    }
}
impl Render for Sidebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Rows the list did not lay out last frame are off screen.
        let laid_out = std::mem::take(&mut self.laid_out);
        self.rows.retain(|id, _| laid_out.contains(id));
        let theme = self.theme;
        let screen = self.core.read(cx).screen().clone();
        let nav = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(
                self.nav_row(
                    "sidebar-new-thread",
                    "new-thread",
                    &self.nav_hover,
                    AssetId::TABLER_EDIT,
                    "New thread",
                    false,
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.new_thread(None, cx))),
            )
            .child(
                self.nav_row(
                    "sidebar-marketplace",
                    "marketplace",
                    &self.nav_hover,
                    AssetId::TABLER_SHOPPING_BAG,
                    "Marketplace",
                    matches!(screen, Screen::Marketplace),
                    cx,
                )
                .on_click(cx.listener(|this, _, _, cx| this.navigate(Screen::Marketplace, cx))),
            );
        let entries = gpui::list(
            self.threads_list.clone(),
            cx.processor(|this, index, _, cx| this.render_entry(index, cx)),
        )
        .flex_1()
        .min_h_0();
        let profile = self.profile.as_ref().map(|profile| {
            let owner = cx.entity().downgrade();
            let origin = self.profile_origin.clone();
            let account = profile.clone();
            let max_usage = (self
                .profile_origin
                .get()
                .unwrap_or_default()
                .top()
                .min(window.viewport_size().height)
                - px(178.0))
            .max(px(0.0));
            account.update(cx, |account, cx| account.set_usage_height(max_usage, cx));
            let content = div()
                .id("artisan-desktop-profile-menu")
                .debug_selector(|| "artisan-desktop-profile-menu".into())
                .w(self
                    .profile_origin
                    .get()
                    .unwrap_or_default()
                    .size
                    .width
                    .max(px(256.0)))
                .child(profile.clone())
                .on_mouse_down_out(move |event, _, cx| {
                    if !origin.contains(&event.position)
                        && !account.read(cx).contains_popup(&event.position)
                    {
                        let _ = owner.update(cx, |this, cx| {
                            this.close_profile(cx);
                        });
                    }
                });
            deferred(
                anchored()
                    .anchor(gpui::Anchor::BottomLeft)
                    .position(self.profile_origin.get().unwrap_or_default().origin)
                    .offset(point(px(0.0), px(-4.0)))
                    .child(animate_menu(
                        div()
                            .id("profile-menu-motion")
                            .relative()
                            .child(editor_ui::hover_card::glass_hover_card(&theme, content)),
                        &self.profile_motion,
                        "profile",
                        MenuEntrance::Scale {
                            origin: point(0.0, 1.0),
                        },
                        PROFILE_MENU_OPEN,
                        PROFILE_MENU_CLOSE,
                    )),
            )
            .with_priority(1)
            .into_any_element()
        });
        let rename = self.rename.as_ref().map(|rename| {
            let owner = cx.entity().downgrade();
            Dialog::new(
                "sidebar-rename",
                theme,
                "Rename thread",
                rename.editor.clone(),
            )
            .on_dismiss(move |_, _, cx| {
                let _ = owner.update(cx, |this, cx| {
                    this.rename = None;
                    cx.notify();
                });
            })
            .into_any_element()
        });
        let picker = self
            .picker
            .as_ref()
            .filter(|_| self.picker_host == self.host_id(cx))
            .map(|(picker, _)| {
                let owner = cx.entity().downgrade();
                let creating = picker.read(cx).creating();
                Dialog::new(
                    "sidebar-add-project",
                    theme,
                    "Add a project",
                    picker.clone(),
                )
                .dismissible(!creating)
                .on_dismiss(move |_, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if !this
                            .picker
                            .as_ref()
                            .is_some_and(|(picker, _)| picker.read(cx).creating())
                        {
                            this.picker = None;
                            cx.notify();
                        }
                    });
                })
                .into_any_element()
            });
        let menu = self.menu.as_ref().map(|menu| self.render_menu(menu, cx));
        let paging = self.host.as_ref().and_then(|host| {
            let page = host.read(cx).thread_page(false);
            let loading = page.loading;
            let more = page.next.is_some();
            let older = !page.at_latest;
            let error = page.error.clone();
            if !more && !older && error.is_none() {
                return None;
            }
            let host = host.clone();
            let latest_host = host.clone();
            Some(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .children(error.map(|error| {
                        div()
                            .text_xs()
                            .text_color(theme.colors.destructive.to_paint())
                            .child(error)
                    }))
                    .children(
                        (older || host.read(cx).thread_page(false).error.is_some()).then(|| {
                            Button::text("sidebar-latest-threads", theme, "Latest threads")
                                .disabled(loading)
                                .variant(ButtonVariant::Ghost)
                                .on_activate(move |_, _, cx| {
                                    latest_host.update(cx, |host, cx| {
                                        host.load_threads(
                                            false,
                                            editor_core::ThreadPageDirection::Latest,
                                            cx,
                                        )
                                    })
                                })
                        }),
                    )
                    .children(more.then(|| {
                        Button::text(
                            "sidebar-older-threads",
                            theme,
                            if loading {
                                "Loading…"
                            } else {
                                "Older threads"
                            },
                        )
                        .disabled(loading)
                        .variant(ButtonVariant::Ghost)
                        .on_activate(cx.listener(move |this, _, _, cx| {
                            this.history_open = true;
                            host.update(cx, |host, cx| {
                                host.load_threads(
                                    false,
                                    editor_core::ThreadPageDirection::Older,
                                    cx,
                                )
                            });
                        }))
                    })),
            )
        });
        let error = self.error.clone().map(|error| {
            div()
                .text_xs()
                .text_color(theme.colors.destructive.to_paint())
                .child(error)
                .child(
                    Button::text("sidebar-error-dismiss", theme, "Dismiss")
                        .variant(ButtonVariant::Ghost)
                        .on_activate(cx.listener(|this, _, _, cx| {
                            this.error = None;
                            cx.notify();
                        })),
                )
        });
        let icon_error = self.icons.values().any(|entry| entry.failed);
        let footer = self.render_profile_trigger(window, cx);
        div()
            .id("sidebar")
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(10.0))
            .child(
                div()
                    .id("artisan-workspace-navigation-hover-surface")
                    .relative()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(self.nav_hover.surface_probe())
                    .child(
                        self.nav_hover
                            .pill("sidebar", &theme, px(6.0), cx.reduce_motion()),
                    )
                    .child(nav)
                    .child(
                        div()
                            .id("sidebar-scroll")
                            .debug_selector(|| "sidebar-threads".into())
                            .relative()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .child({
                                let owner = cx.entity().downgrade();
                                gpui::canvas(
                                    |_, _, _| {},
                                    move |bounds, (), window, cx| {
                                        let height = f32::from(bounds.size.height);
                                        let _ = owner.update(cx, |this, cx| {
                                            if (this.threads_height - height).abs() > 0.1 {
                                                this.threads_height = height;
                                                if let Some(index) =
                                                    this.entries.iter().position(|entry| {
                                                        matches!(entry, Entry::Spacer)
                                                    })
                                                {
                                                    this.threads_list
                                                        .remeasure_items(index..index + 1);
                                                }
                                                cx.notify();
                                                window.refresh();
                                            }
                                        });
                                    },
                                )
                                .absolute()
                                .size_full()
                            })
                            .child(self.threads_hover.surface_probe())
                            .child(self.threads_hover.pill(
                                "sidebar-threads",
                                &theme,
                                px(6.0),
                                cx.reduce_motion(),
                            ))
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if *hovered {
                                    this.nav_hover.hide();
                                } else {
                                    this.threads_hover.clear();
                                }
                                cx.notify();
                            }))
                            .child(entries),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_none()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .h(px(1.0))
                                    .mx(px(-10.0))
                                    .bg(theme.colors.line.to_paint())
                                    .debug_selector(|| "artisan-sidebar-footer-divider".into()),
                            )
                            .child(footer),
                    )
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if !*hovered {
                            this.nav_hover.clear();
                            cx.notify();
                        }
                    })),
            )
            .children(paging)
            .children(error)
            .children(icon_error.then(|| {
                Button::text("sidebar-metadata-retry", theme, "Retry project icons")
                    .variant(ButtonVariant::Ghost)
                    .on_activate(cx.listener(|this, _, _, cx| {
                        for entry in this.icons.values_mut().filter(|entry| entry.failed) {
                            entry.checked = Instant::now() - Duration::from_secs(5);
                        }
                        cx.notify();
                    }))
            }))
            .children(menu)
            .children(rename)
            .children(picker)
            .children(profile)
    }
}
