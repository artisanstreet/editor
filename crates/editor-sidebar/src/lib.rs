//! The sidebar renders a bounded Forge thread projection. Only visible rows load metadata.
mod avatar;
mod list;
mod picker;
mod rail_icon;
mod row;
mod settings_nav;
#[cfg(test)]
mod tests;
mod usage;
mod usage_page;
mod usage_visual;
use crate::list::{Lists, RowData};
use crate::picker::{FolderPicker, PickerEvent};
use crate::rail_icon::{RAIL_ICON_SIZE, RailGlyph, RailIcon};
pub use crate::usage_page::UsagePage;
use artisan_api::{ProjectId, ThreadId, Timestamp, UpdateThread};
use editor_core::{Core, CoreEvent, DraggedThread, HostData, HostEvent, HostId, Screen};
use editor_ui::asset_seam::asset_glyph;
use editor_ui::hover_card::{HoverFill, hover_fill};
use editor_ui::measure::Measured;
use editor_ui::motion::MotionDuration;
use editor_ui::picker_motion::HoverSurface;
use editor_ui::picker_motion::{MenuEntrance, MenuMotion, animate_menu};
use editor_ui::theme::{ActiveTheme as _, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use editor_ui::{
    AssetId,
    button::{Button, ButtonVariant},
    dialog::Dialog,
    popover::Popover,
    text_editor::{TextEditor, TextEditorEvent},
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, Focusable as _, Image,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point, Render,
    ScrollWheelEvent, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, anchored, deferred, div, point, px,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};
/// How close to an edge of the loaded threads the reader gets before the
/// next page loads, as in the conversation.
const READ_AHEAD_PX: f32 = 600.0;
/// The window frame's left rail, as wide as the titlebar is tall, so the
/// frame wraps the content card equally on the left and the top.
pub const RAIL_WIDTH: f32 = 48.0;
/// The avatar button's inset from the rail's edges and the window's bottom.
const RAIL_AVATAR_INSET: f32 = 4.0;
/// A rail destination's square, its 18 px icon centred.
const RAIL_BUTTON_SIZE: f32 = 32.0;
/// The hairline above History, and the space between it and the row.
const HISTORY_SEPARATOR: f32 = 1.0;
const HISTORY_SEPARATOR_GAP: f32 = 8.0;
const PROFILE_MENU_OPEN: Duration = MotionDuration::Fast.as_duration();
const PROFILE_MENU_CLOSE: Duration = MotionDuration::Quick.as_duration();
use std::{
    collections::{HashMap, HashSet},
    future::Future,
    time::Duration,
};

#[derive(Clone)]
enum Entry {
    Thread(RowData),
    History,
    Heading(&'static str),
    Gap(f32),
    Spacer,
}
/// What identifies each entry across rebuilds. A gap is the space above
/// the entry after it.
fn entry_keys(entries: &[Entry]) -> Vec<String> {
    let mut keys = vec![String::new(); entries.len()];
    let mut below = String::from("end");
    for (index, entry) in entries.iter().enumerate().rev() {
        keys[index] = match entry {
            Entry::Thread(row) => format!("thread:{}", row.id),
            Entry::History => "history".into(),
            Entry::Spacer => "spacer".into(),
            Entry::Heading(label) => format!("heading:{label}"),
            Entry::Gap(height) => format!("gap:{height}:{below}"),
        };
        below.clone_from(&keys[index]);
    }
    keys
}

/// Tells `list` which runs of entries came and went between `old` and
/// `new`; whether anything did.
fn splice_entries(list: &gpui::ListState, old: &[Entry], new: &[Entry]) -> bool {
    let (old, new) = (entry_keys(old), entry_keys(new));
    let old_set: std::collections::HashSet<&String> = old.iter().collect();
    let new_set: std::collections::HashSet<&String> = new.iter().collect();
    let (mut i, mut j, mut changed) = (0, 0, false);
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            i += 1;
            j += 1;
            continue;
        }
        changed = true;
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
        // Entries changed order: replace the rest.
        list.splice(j..j + (old.len() - i), new.len() - j);
        break;
    }
    changed
}
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent view flags (scroll, glide, history, profile), not a state machine"
)]
pub struct Sidebar {
    core: Entity<Core>,
    host: Option<Entity<HostData>>,
    open: Option<ThreadId>,
    lists: Lists,
    entries: Vec<Entry>,
    threads_list: gpui::ListState,
    threads_height: f32,
    /// The thread list's scroll edges, top and bottom.
    threads_edges: [editor_ui::scroll_edge::EdgeFade; 2],
    /// Whether the loaded threads reach the newest one.
    at_latest: bool,
    threads_glide: editor_ui::wheel_glide::WheelGlide,
    threads_glide_scheduled: bool,
    history_open: bool,
    nav_hover: HoverSurface,
    threads_hover: HoverSurface,
    menu_hover: HoverSurface,
    /// Cached thread rows the list laid out since the last render; the
    /// rest are dropped, so only rows on screen keep an entity.
    rows: HashMap<ThreadId, (Entity<row::ThreadRow>, Measured)>,
    laid_out: HashSet<ThreadId>,
    profile_origin: Measured,
    avatars: HashMap<(editor_ui::theme::ThemeMode, String), Arc<Image>>,
    profile: Option<Entity<usage::Account>>,
    profile_open: bool,
    profile_motion: Rc<RefCell<MenuMotion>>,
    profile_settle: Option<Task<()>>,
    /// The rail's New thread, Marketplace and Usage icons.
    rail_icons: [Entity<RailIcon>; 3],
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
    host_events: Option<Subscription>,
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
            host: None,
            open: None,
            lists: Lists::default(),
            entries: Vec::new(),
            // Measuring ahead of the view lets the wheel reach past what
            // was drawn, and is where the next page starts loading.
            threads_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(READ_AHEAD_PX)),
            threads_height: 0.0,
            threads_edges: Default::default(),
            at_latest: true,
            threads_glide: editor_ui::wheel_glide::WheelGlide::default(),
            threads_glide_scheduled: false,
            history_open: false,
            nav_hover: HoverSurface::default(),
            threads_hover: HoverSurface::default(),
            menu_hover: HoverSurface::default(),
            rows: HashMap::new(),
            laid_out: HashSet::new(),
            profile_origin: Measured::default(),
            avatars: HashMap::new(),
            profile: None,
            profile_open: false,
            profile_motion: Rc::default(),
            profile_settle: None,
            rail_icons: [RailGlyph::Pen, RailGlyph::Bag, RailGlyph::Bars]
                .map(|glyph| cx.new(|_| RailIcon::new(glyph))),
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
            host_events: None,
        };
        this.sync(cx);
        this
    }
    pub const fn core(&self) -> &Entity<Core> {
        &self.core
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let host = core.active_host().cloned();
        let open = match core.screen() {
            Screen::Thread { thread, .. } => Some(thread.clone()),
            Screen::Home
            | Screen::NewThread { .. }
            | Screen::Settings(_)
            | Screen::Marketplace
            | Screen::Usage => None,
        };
        if host.as_ref().map(Entity::entity_id) != self.host.as_ref().map(Entity::entity_id) {
            self.host_events = host.as_ref().map(|host| {
                cx.subscribe(host, |this, _, event: &HostEvent, cx| match event {
                    HostEvent::Changed | HostEvent::ThreadChanged { .. } | HostEvent::Reset => {
                        this.refresh(cx);
                    }
                    HostEvent::ImagesChanged | HostEvent::EngineInstall { .. } => cx.notify(),
                    HostEvent::ThreadSummaryChanged { .. }
                    | HostEvent::WslDistro { .. }
                    | HostEvent::Item(_)
                    | HostEvent::Live { .. }
                    | HostEvent::EditorCall { .. } => {}
                })
            });
            self.host = host;
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
        let now = artisan_api::now();
        self.lists = self
            .host
            .as_ref()
            .map(|host| list::lists(host.read(cx), self.open.as_ref(), now))
            .unwrap_or_default();
        self.at_latest = self
            .host
            .as_ref()
            .is_none_or(|host| host.read(cx).thread_page(false).at_latest);
        self.rebuild_entries();
        self.schedule_tick(now, cx);
        self.slide_threads(false, cx);
        cx.notify();
    }
    /// Slides the loaded threads with the reader, like the conversation's
    /// window: the list's end loads older threads, the top loads the newer
    /// ones paging back dropped. A failed page retries only when the
    /// reader scrolls to its edge again (`scrolled`), never by itself.
    fn slide_threads(&self, scrolled: bool, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        if self.threads_height <= 0.0 {
            return;
        }
        // Rows have fixed heights: the list only knows the ones it drew,
        // which would put both edges in reach at once.
        let top = self.threads_list.logical_scroll_top();
        let height = |entries: &[Entry]| -> f32 { entries.iter().map(Self::entry_height).sum() };
        let above = height(&self.entries[..top.item_ix.min(self.entries.len())])
            + f32::from(top.offset_in_item);
        let below = height(&self.entries) - above - self.threads_height;
        let page = host.read(cx).thread_page(false);
        if page.loading || !page.loaded || (page.error.is_some() && !scrolled) {
            return;
        }
        let direction = if !page.at_latest && above < READ_AHEAD_PX {
            editor_core::ThreadPageDirection::Newer
        } else if page.next.is_some()
            && below < READ_AHEAD_PX
            // Older threads land in History; closed, it ends the list.
            && (self.history_open || self.lists.history.is_empty())
        {
            editor_core::ThreadPageDirection::Older
        } else {
            return;
        };
        host.update(cx, |host, cx| host.load_threads(false, direction, cx));
    }
    fn rebuild_entries(&mut self) {
        let previous = std::mem::take(&mut self.entries);
        let top = self.threads_list.logical_scroll_top();
        let at_top = top.item_ix == 0 && top.offset_in_item == px(0.0);
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
        // Each run that came or went is its own splice, so the window
        // sliding at both ends keeps the reader's place.
        if splice_entries(&self.threads_list, &previous, &self.entries) {
            // Read from the top of the newest threads, new ones show up
            // where the reader is instead of pushing the list down.
            if at_top && self.at_latest {
                self.threads_list.scroll_to(gpui::ListOffset::default());
            }
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
    fn entry_height(entry: &Entry) -> f32 {
        match entry {
            Entry::Thread(_) => row::ROW_HEIGHT,
            Entry::History => HISTORY_SEPARATOR + HISTORY_SEPARATOR_GAP + 34.0,
            Entry::Heading(_) => 21.0,
            Entry::Gap(height) => *height,
            Entry::Spacer => 0.0,
        }
    }
    /// The thread list's scroll edges, shown while threads run past them.
    fn threads_edges(&self, theme: &editor_ui::theme::ArtisanTheme) -> [AnyElement; 2] {
        use editor_ui::scroll_edge::{Edge, hidden_above, hidden_below, scroll_edge};
        let color = theme.colors.background;
        let (above, below) = (self.threads_list.clone(), self.threads_list.clone());
        [
            scroll_edge(Edge::Top, color, &self.threads_edges[0], move || {
                hidden_above(&above)
            })
            .top_0()
            .into_any_element(),
            scroll_edge(Edge::Bottom, color, &self.threads_edges[1], move || {
                hidden_below(&below)
            })
            .bottom_0()
            .into_any_element(),
        ]
    }
    fn history_spacer(&self) -> f32 {
        (self.threads_height - self.entries.iter().map(Self::entry_height).sum::<f32>()).max(12.0)
    }
    fn threads_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = self.threads_list.clone();
        self.slide_threads(true, cx);
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
    fn schedule_threads_glide(&mut self, window: &mut Window, cx: &Context<Self>) {
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
                this.slide_threads(true, cx);
                cx.notify();
            }
            this.schedule_threads_glide(window, cx);
        });
    }
    fn schedule_tick(&mut self, now: Timestamp, cx: &Context<Self>) {
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
            this.update(cx, |this, cx| {
                this.tick = None;
                this.refresh(cx);
            })
            .ok();
        });
        self.tick = Some((at, task));
    }
    /// Runs a Forge call; a failure is logged and shown, never silent.
    #[cfg_attr(
        not(test),
        expect(
            clippy::needless_pass_by_ref_mut,
            reason = "test builds take the completion gate out of self"
        )
    )]
    fn call<T: 'static>(
        &mut self,
        cx: &Context<Self>,
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
                sidebar
                    .update(cx, |sidebar, _| sidebar.call_completions += 1)
                    .ok();
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

    pub(crate) fn open_thread(&self, thread: ThreadId, cx: &mut Context<Self>) {
        if let Some(host) = self.host_id(cx) {
            self.navigate(Screen::Thread { host, thread }, cx);
        }
    }

    fn new_thread(&self, project: Option<ProjectId>, cx: &mut Context<Self>) {
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
        self.menu_hover.clear();
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
        cx: &Context<Self>,
    ) {
        let Some(host) = &self.host else {
            return;
        };
        let client = host.read(cx).client.clone();
        self.call(cx, format!("{what} thread {thread}"), async move {
            client.update_thread(&thread, &update).await
        });
    }

    fn mark_unread(&mut self, thread: ThreadId, cx: &Context<Self>) {
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
                overlay_window
                    .update(cx, |_, window, cx| {
                        editor_ui::overlay::dismiss("sidebar-rename", window, cx);
                    })
                    .ok();
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
    fn settle(&mut self, thread: &ThreadId, cx: &Context<Self>) {
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
        let picker = cx.new(|cx| FolderPicker::new(client, cx));
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
    fn load_visible_projects(&self, projects: &[ProjectId], cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        if !host.read(cx).is_live() {
            return;
        }
        host.update(cx, |host, cx| {
            for project in projects {
                host.load_repository(project, false, cx);
                host.load_project_icon(project, cx);
            }
        });
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
            let (sidebar, hover) = (cx.entity().downgrade(), self.threads_hover.clone());
            let row_bounds = bounds.clone();
            let row = cx.new(|_| row::ThreadRow::new(inputs, sidebar, hover, row_bounds));
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
            .debug_selector(move || selector)
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

    #[expect(
        clippy::too_many_lines,
        reason = "one match over every list entry kind"
    )]
    fn render_entry(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = self.entries.get(index).cloned() else {
            return div().into_any_element();
        };
        // The History separator runs edge to edge; everything else keeps the
        // column's inset.
        let inset = !matches!(entry, Entry::History);
        let content = match entry {
            Entry::Thread(data) => {
                self.load_visible_projects(std::slice::from_ref(&data.project), cx);
                let image = self
                    .host
                    .as_ref()
                    .and_then(|host| host.read(cx).project_icon(&data.project));
                let repository = self
                    .host
                    .as_ref()
                    .and_then(|host| host.read(cx).repository(&data.project))
                    .map(|repository| repository.host.clone());
                let drag = self.host.as_ref().and_then(|host| {
                    let host = host.read(cx);
                    let title = host.threads.get(&data.id)?.title.clone()?;
                    artisan_mentions::valid_thread_title(&title).then(|| DraggedThread {
                        host: host.id.clone(),
                        thread: data.id.clone(),
                        title,
                    })
                });
                let inputs = row::RowInputs {
                    data,
                    image,
                    host: repository,
                    reduced: cx.reduce_motion(),
                    mode: cx.theme().colors.mode,
                    drag,
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
                .text_color(cx.theme().colors.text_muted.to_paint())
                .child(label)
                .into_any_element(),
            // The separator is part of the entry, so it slides with the row
            // when History opens.
            Entry::History => div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .mb(px(HISTORY_SEPARATOR_GAP))
                        .h(px(HISTORY_SEPARATOR))
                        .bg(cx.theme().colors.line.to_paint()),
                )
                .child(
                    div().px(px(8.0)).child(
                        Self::nav_row(
                            "sidebar-history",
                            "history",
                            &self.threads_hover,
                            AssetId::TABLER_ARCHIVE,
                            "History",
                            false,
                            cx,
                        )
                        .child(div().flex_1())
                        .child(
                            asset_glyph(if self.history_open {
                                AssetId::TABLER_CHEVRON_DOWN
                            } else {
                                AssetId::TABLER_CHEVRON_RIGHT
                            })
                            .size(px(12.0))
                            .text_color(cx.theme().colors.text_muted.to_paint()),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history_open = !this.history_open;
                            // Closed, History's pages are out of view: the window
                            // returns to the newest threads in one page.
                            if !this.history_open
                                && let Some(host) = &this.host
                                && !host.read(cx).thread_page(false).at_latest
                            {
                                host.update(cx, |host, cx| {
                                    host.load_threads(
                                        false,
                                        editor_core::ThreadPageDirection::Latest,
                                        cx,
                                    );
                                });
                            }
                            this.rebuild_entries();
                            this.slide_threads(false, cx);
                            cx.notify();
                        })),
                    ),
                )
                .into_any_element(),
        };
        div()
            .id(gpui::SharedString::from(format!("sidebar-entry-{index}")))
            .w_full()
            .when(inset, |entry| entry.px(px(8.0)))
            .child(content)
            .on_scroll_wheel(cx.listener(Self::threads_wheel))
            .into_any_element()
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one element tree for the thread and project menus"
    )]
    fn render_menu(&self, menu: &Menu, cx: &Context<Self>) -> AnyElement {
        let MenuTarget::Thread(thread) = menu.target.clone();
        let theme = *cx.theme();
        let item = |id: &'static str, asset: AssetId, label: &'static str, destructive: bool| {
            editor_ui::popover::menu_item(
                id,
                &theme,
                &self.menu_hover,
                Some(asset),
                label,
                destructive,
            )
        };
        let mut content =
            editor_ui::popover::menu_rows(&self.menu_hover, &theme, cx.reduce_motion())
                .w(px(198.0));
        let settleable = self
            .lists
            .inbox
            .iter()
            .any(|row| row.id == thread && row.settleable);
        if settleable {
            let thread = thread.clone();
            content = content
                .child(
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
                )
                .child(editor_ui::glass::glass_menu_separator(&theme));
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
        content = content.child(editor_ui::glass::glass_menu_separator(&theme));
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
                owner
                    .update(cx, |this, cx| {
                        this.menu = None;
                        cx.notify();
                    })
                    .ok();
            },
        )
        .menu_padding(px(4.0))
        .quiet_glass_menu()
        .into_any_element()
    }
    fn nav_row(
        id: impl Into<SharedString>,
        hover_id: impl Into<SharedString>,
        surface: &HoverSurface,
        icon: AssetId,
        label: impl Into<SharedString>,
        current: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let (id, hover_id, label) = (id.into(), hover_id.into(), label.into());
        let hover = surface.clone();
        let selector = id.clone();
        div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(label.clone())
            .aria_selected(current)
            .focusable()
            .tab_index(0)
            .debug_selector(move || selector.to_string())
            .relative()
            .w_full()
            .h(px(34.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .cursor_pointer()
            .child(surface.row_probe(hover_id.to_string()))
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                hover.hovered(&hover_id, *hovered);
                cx.notify();
            }))
            .child(
                div()
                    .size(px(16.0))
                    .flex_none()
                    .text_color(cx.theme().colors.text_muted.to_paint())
                    .child(asset_glyph(icon).size(px(15.0))),
            )
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .text_color(if current {
                        cx.theme().colors.link.to_paint()
                    } else {
                        cx.theme().colors.text.to_paint()
                    })
                    .child(label),
            )
    }

    /// The rail's avatar, in the window frame's bottom-left corner: it
    /// opens the account and usage menu. Who and where are in the menu and
    /// the tooltip-free label; a status dot says the host needs attention.
    fn render_profile_trigger(&mut self, cx: &Context<Self>) -> AnyElement {
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
                let status = host.status_line().map(str::to_owned);
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
        let theme = *cx.theme();
        let image = Arc::clone(
            self.avatars
                .entry((theme.colors.mode, seed.clone()))
                .or_insert_with(|| avatar::avatar_image(&seed)),
        );
        let size = px(2.0f32.mul_add(-RAIL_AVATAR_INSET, RAIL_WIDTH));
        div()
            .id("artisan-desktop-profile-trigger")
            .role(gpui::Role::Button)
            .aria_label(format!("{name} on {machine}: account and usage"))
            .aria_expanded(self.profile_open)
            .focusable()
            .tab_index(0)
            .debug_selector(|| "artisan-desktop-profile-trigger".into())
            .relative()
            .size(size)
            .flex_shrink_0()
            .p(px(4.0))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .cursor_pointer()
            .hover(|style| hover_fill(style, &theme, HoverFill::Plain))
            .when(self.profile_open, |trigger| {
                hover_fill(trigger, &theme, HoverFill::Plain)
            })
            .child(self.profile_origin.record())
            .child(
                div()
                    .size_full()
                    .rounded(RadiusTokens::value(RadiusStep::Md))
                    .overflow_hidden()
                    .child(avatar::avatar(image)),
            )
            .children(status.is_some().then(|| {
                // Ringed in the frame's colour, so it reads cut out of the
                // avatar's corner.
                div()
                    .absolute()
                    .right(px(1.0))
                    .bottom(px(1.0))
                    .size(px(10.0))
                    .rounded_full()
                    .border_2()
                    .border_color(theme.colors.frame.to_paint())
                    .bg(theme.colors.banner_warning.text.to_paint())
            }))
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
                    owner.update(cx, Self::close_profile).ok();
                })
            }));
        }
        self.profile_open = true;
        let generation = self.profile_motion.borrow_mut().begin_open();
        self.settle_profile(generation, true, cx);
        cx.notify();
    }
    fn close_profile(&mut self, cx: &mut Context<Self>) {
        if !self.profile_open {
            return;
        }
        self.profile_open = false;
        if let Some(account) = &self.profile {
            account.update(cx, usage::Account::clear_interaction);
        }
        let generation = self.profile_motion.borrow_mut().begin_close();
        self.settle_profile(generation, false, cx);
        cx.notify();
    }
    fn settle_profile(&mut self, generation: u64, open: bool, cx: &Context<Self>) {
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
            this.update(cx, |this, cx| {
                if settle(this) {
                    this.profile_settle = None;
                    cx.notify();
                }
            })
            .ok();
        }));
    }
}
impl Render for Sidebar {
    #[expect(
        clippy::too_many_lines,
        reason = "the sidebar's root element tree, ported from the old Editor"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Rows the list did not lay out last frame are off screen.
        let laid_out = std::mem::take(&mut self.laid_out);
        self.rows.retain(|id, _| laid_out.contains(id));
        let theme = *cx.theme();
        let screen = self.core.read(cx).screen().clone();
        let settings_page = match screen {
            Screen::Settings(page) => Some(page),
            Screen::Home
            | Screen::Thread { .. }
            | Screen::NewThread { .. }
            | Screen::Marketplace
            | Screen::Usage => None,
        };
        // The rail's destinations, above the avatar.
        let [pen, bag, bars] = self.rail_icons.clone();
        let destinations = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(4.0))
            .child(
                rail_button(
                    &theme,
                    "sidebar-new-thread",
                    &pen,
                    "New thread",
                    matches!(screen, Screen::NewThread { .. }),
                )
                .on_click(cx.listener(|this, _, _, cx| this.new_thread(None, cx))),
            )
            .child(
                rail_button(
                    &theme,
                    "sidebar-marketplace",
                    &bag,
                    "Marketplace",
                    matches!(screen, Screen::Marketplace),
                )
                .on_click(cx.listener(|this, _, _, cx| this.navigate(Screen::Marketplace, cx))),
            )
            .child(
                rail_button(
                    &theme,
                    "sidebar-usage",
                    &bars,
                    "Usage",
                    matches!(screen, Screen::Usage),
                )
                .on_click(cx.listener(|this, _, _, cx| this.navigate(Screen::Usage, cx))),
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
                        && !account.read(cx).contains_popup(event.position)
                    {
                        owner
                            .update(cx, |this, cx| {
                                this.close_profile(cx);
                            })
                            .ok();
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
                            // The menu floats over thread rows: clicks on it
                            // must not reach the rows beneath.
                            .occlude()
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
                owner
                    .update(cx, |this, cx| {
                        this.rename = None;
                        cx.notify();
                    })
                    .ok();
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
                    owner
                        .update(cx, |this, cx| {
                            if !this
                                .picker
                                .as_ref()
                                .is_some_and(|(picker, _)| picker.read(cx).creating())
                            {
                                this.picker = None;
                                cx.notify();
                            }
                        })
                        .ok();
                })
                .into_any_element()
            });
        let menu = self.menu.as_ref().map(|menu| self.render_menu(menu, cx));
        // Paging follows the scroll; only a failed page says anything.
        let paging = self.host.as_ref().and_then(|host| {
            // Offline, the host row already says so.
            let error = host
                .read(cx)
                .thread_page(false)
                .error
                .clone()
                .filter(|_| host.read(cx).is_live())?;
            Some(
                div()
                    .text_xs()
                    .text_color(theme.colors.destructive_text.to_paint())
                    .child(error),
            )
        });
        let error = self.error.clone().map(|error| {
            div()
                .text_xs()
                .text_color(theme.colors.destructive_text.to_paint())
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
        let icon_error = self
            .host
            .as_ref()
            .is_some_and(|host| host.read(cx).project_icons_failed());
        let rail = div()
            .w(px(RAIL_WIDTH))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .justify_between()
            .items_center()
            .py(px(RAIL_AVATAR_INSET))
            .debug_selector(|| "artisan-desktop-rail".into())
            .child(destinations)
            .child(self.render_profile_trigger(cx));
        // Settings lists its pages where the threads were.
        let threads = settings_page.is_none().then(|| {
            div()
                .id("sidebar-scroll")
                .debug_selector(|| "sidebar-threads".into())
                .relative()
                // Spans the column's padding so the History separator meets
                // its edges; entries put the inset back.
                .mx(px(-8.0))
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
                            owner.update(cx, |this, cx| {
                                if (this.threads_height - height).abs() > 0.1 {
                                    this.threads_height = height;
                                    if let Some(index) = this
                                        .entries
                                        .iter()
                                        .position(|entry| matches!(entry, Entry::Spacer))
                                    {
                                        this.threads_list.remeasure_items(index..index + 1);
                                    }
                                    cx.notify();
                                    window.refresh();
                                }
                            }).ok();
                        },
                    )
                    .absolute()
                    .size_full()
                })
                .child(self.threads_hover.surface_probe())
                .child(self.threads_hover.pill(
                    "sidebar-threads",
                    &theme,
                    px(8.0),
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
                .child(entries)
                .children(self.threads_edges(&theme))
        });
        let column = div()
            .id("sidebar")
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .min_h(px(0.0))
            .overflow_hidden()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(8.0))
            // The last row sits closer to the card's edge than the
            // separator above it, so History doesn't float.
            .pb(px(4.0))
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
                            .pill("sidebar", &theme, px(8.0), cx.reduce_motion()),
                    )
                    .children(settings_page.map(|page| {
                        div()
                            .id("sidebar-settings")
                            .debug_selector(|| "sidebar-settings".into())
                            // Spans the column's padding so the separator
                            // under Back meets its edges.
                            .mx(px(-8.0))
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(self.render_settings_nav(page, cx))
                    }))
                    .children(threads)
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
                        if let Some(host) = this.host.clone() {
                            host.update(cx, editor_core::HostData::retry_project_icons);
                        }
                    }))
            }));
        // The rail sits on the window frame, the column in the content
        // card the workspace paints behind it.
        div()
            .size_full()
            .flex()
            .flex_row()
            .child(rail)
            .child(column)
            .children(menu)
            .children(rename)
            .children(picker)
            .children(profile)
    }
}

/// One of the rail's destinations: a 32 px icon button, the icon in the
/// text colour while its screen is open, its name in a tooltip beside it.
fn rail_button(
    theme: &editor_ui::theme::ArtisanTheme,
    id: &'static str,
    icon: &Entity<RailIcon>,
    label: &'static str,
    current: bool,
) -> Stateful<Div> {
    let theme = *theme;
    let play = icon.clone();
    let anchor = editor_ui::hover_card::TooltipAnchor::default();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .aria_selected(current)
        .focusable()
        .tab_index(0)
        .debug_selector(move || id.to_owned())
        .relative()
        .size(px(RAIL_BUTTON_SIZE))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(RadiusTokens::value(RadiusStep::Lg))
        .cursor_pointer()
        .text_color(if current {
            theme.colors.text.to_paint()
        } else {
            theme.colors.text_muted.to_paint()
        })
        .hover(move |style| {
            hover_fill(style, &theme, HoverFill::Plain).text_color(theme.colors.text.to_paint())
        })
        .child(anchor.probe())
        .on_hover(move |hovered, _, cx| {
            if *hovered {
                play.update(cx, RailIcon::play);
            }
        })
        .child(
            icon.clone()
                .cached(gpui::StyleRefinement::default().size(px(RAIL_ICON_SIZE))),
        )
        .tooltip(move |_, cx| {
            anchor.tooltip(
                move |_, _| {
                    editor_ui::hover_card::glass_hover_card(
                        &theme,
                        div()
                            .px(px(10.0))
                            .py(px(6.0))
                            .text_size(TypographyTokens::size(TextStep::Sm))
                            .text_color(theme.colors.text.to_paint())
                            .child(label),
                    )
                    .into_any_element()
                },
                cx,
            )
        })
}
