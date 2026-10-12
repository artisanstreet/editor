//! The left sidebar, ported from the previous Editor's `desktop_sidebar`:
//! New thread and Marketplace, the thread inbox with History collapsed at
//! its foot, and the account footer that switches hosts and opens Settings.
//!
//! UI state only; every list is read from the active host's [`HostData`]
//! and changed by calling Forge, whose events then update the lists.

mod avatar;
mod folder_dialog;
mod hover;
mod list;
mod picker;
mod row;
#[cfg(test)]
mod tests;
mod usage;

use crate::hover::HoverSurface;
use crate::list::{Lists, RowData, RowState};
use crate::picker::{FolderPicker, PickerEvent};
use crate::row::{ROW_HEIGHT, ThreadRow};
use anyhow::Context as _;
use artisan_api::{CreateProject, EngineId, ProjectId, Seq, ThreadId, Timestamp, UpdateThread};
use editor_core::{
    ConnectionStatus, Core, CoreEvent, HostData, HostEvent, HostId, Screen, SettingsPage,
};
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::dialog::Dialog;
use editor_ui::glass::{
    GlassStrength, glass_badge, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use editor_ui::gradient::hover_fill_gradient;
use editor_ui::hover_card::hover_highlight_shadow;
use editor_ui::motion::{MotionDuration, MotionPolicy};
use editor_ui::picker_motion::{MenuEntrance, MenuMotion, animate_menu, smooth_out};
use editor_ui::text_editor::{TextEditor, TextEditorEvent};
use editor_ui::theme::{ArtisanTheme, DesktopTheme, Oklch, RadiusStep, RadiusTokens, ThemeMode};
use editor_ui::wheel_glide::WheelGlide;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, FocusHandle, Focusable as _,
    FontWeight, Image, ImageFormat, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Point, Render, ScrollHandle, ScrollWheelEvent, SharedString, Stateful,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Task, WeakEntity,
    Window, anchored, deferred, div, point, px,
};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::future::Future;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The account menu's entrance and exit (`PROFILE_MENU_OPEN`/`CLOSE`).
const PROFILE_MENU_OPEN: Duration = MotionDuration::Fast.as_duration();
const PROFILE_MENU_CLOSE: Duration = MotionDuration::Quick.as_duration();
/// The menu keeps this clear of the window's top edge.
const PROFILE_MENU_VIEWPORT_MARGIN: f32 = 8.0;
/// The menu's fixed rows (header, rules, actions); only usage scrolls.
const PROFILE_MENU_CHROME: f32 = 68.0 + 18.0 + 80.0;
/// The sidebar's hover pill ids.
const NEW_THREAD_HOVER_ID: &str = "new-thread";
const MARKETPLACE_HOVER_ID: &str = "marketplace";
const PROFILE_HOVER_ID: &str = "profile";
const HISTORY_HOVER_ID: &str = "history";

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent disclosures and a frame flag; packing them would conflate them"
)]
pub struct Sidebar {
    core: Entity<Core>,
    theme: ArtisanTheme,
    host: Option<Entity<HostData>>,
    /// The open thread on the active host.
    open: Option<ThreadId>,
    /// The open thread's unseen outcome when it opened, whose dot stays
    /// until the user leaves it.
    held: Option<(ThreadId, RowState)>,
    /// The last activity of the open thread we told Forge was read.
    marked: Option<(ThreadId, Seq)>,
    lists: Lists,
    rows: HashMap<ThreadId, Entity<ThreadRow>>,
    /// Project icons; present once requested, `None` while loading or when
    /// the project has none.
    icons: HashMap<ProjectId, Option<Arc<Image>>>,
    history_open: bool,
    /// The account menu above the footer, and its host submenu beside it.
    profile_open: bool,
    machines_open: bool,
    /// The host row and the account menu, which place the submenu, and the
    /// submenu, which a click inside must not close the menu from.
    machines_trigger: Rc<Cell<gpui::Bounds<Pixels>>>,
    profile_panel: Rc<Cell<gpui::Bounds<Pixels>>>,
    machines_panel: Rc<Cell<gpui::Bounds<Pixels>>>,
    profile_motion: Rc<RefCell<MenuMotion>>,
    profile_settle: Option<Task<()>>,
    /// The footer's bounds, which the menu takes its width and corner from.
    profile_origin: Rc<Cell<gpui::Bounds<Pixels>>>,
    chevron_toggled: Option<Instant>,
    /// The sliding pills: New thread, Marketplace and the footer share one;
    /// the thread list and the account menu have their own.
    nav_hover: HoverSurface,
    threads_hover: HoverSurface,
    profile_hover: HoverSurface,
    /// The thread list's offset, and the wheel ticks gliding toward theirs.
    threads_scroll: ScrollHandle,
    threads_glide: WheelGlide,
    threads_glide_scheduled: bool,
    machines_hover: HoverSurface,
    usage: usage::UsageUi,
    /// Host avatars, drawn once per seed.
    avatars: HashMap<String, Arc<Image>>,
    menu: Option<Menu>,
    rename: Option<Rename>,
    picker: Option<(Entity<FolderPicker>, Subscription)>,
    dialog_focus: FocusHandle,
    /// The last failed action, until dismissed.
    error: Option<String>,
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
    pub fn new(core: Entity<Core>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&core, |sidebar, _, event: &CoreEvent, cx| match event {
            CoreEvent::ScreenChanged | CoreEvent::HostsChanged => sidebar.sync(cx),
            CoreEvent::SettingsChanged => {}
        });
        let mut sidebar = Self {
            core,
            theme: ArtisanTheme::for_mode(ThemeMode::Dark),
            host: None,
            open: None,
            held: None,
            marked: None,
            lists: Lists::default(),
            rows: HashMap::new(),
            icons: HashMap::new(),
            history_open: false,
            profile_open: false,
            machines_open: false,
            machines_trigger: Rc::default(),
            profile_panel: Rc::default(),
            machines_panel: Rc::default(),
            profile_motion: Rc::default(),
            profile_settle: None,
            profile_origin: Rc::default(),
            chevron_toggled: None,
            nav_hover: HoverSurface::default(),
            threads_hover: HoverSurface::default(),
            profile_hover: HoverSurface::default(),
            threads_scroll: ScrollHandle::new(),
            threads_glide: WheelGlide::default(),
            threads_glide_scheduled: false,
            machines_hover: HoverSurface::default(),
            usage: usage::UsageUi::default(),
            avatars: HashMap::new(),
            menu: None,
            rename: None,
            picker: None,
            dialog_focus: cx.focus_handle(),
            error: None,
            tick: None,
            _core: subscription,
            _host: None,
        };
        sidebar.sync(cx);
        sidebar
    }

    #[must_use]
    pub fn core(&self) -> &Entity<Core> {
        &self.core
    }

    /// Follows the active host and the open thread.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let host = core.active_host().cloned();
        let open = host.as_ref().and_then(|host| {
            let id = &host.read(cx).id;
            match core.screen() {
                Screen::Thread { host, thread } if host == id => Some(thread.clone()),
                _ => None,
            }
        });
        if host.as_ref().map(Entity::entity_id) != self.host.as_ref().map(Entity::entity_id) {
            self._host = host.as_ref().map(|host| {
                cx.subscribe(host, |sidebar, _, event: &HostEvent, cx| {
                    if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                        sidebar.refresh(cx);
                    }
                })
            });
            self.host = host;
            self.rows.clear();
            self.lists = Lists::default();
            self.menu = None;
        }
        if open != self.open {
            self.held = open.as_ref().and_then(|id| {
                let row = self.lists.inbox.iter().find(|row| &row.id == id)?;
                matches!(row.state, RowState::Unread | RowState::Failed)
                    .then(|| (id.clone(), row.state))
            });
            self.open = open;
        }
        self.refresh(cx);
    }

    /// Rebuilds the lists from the host, updating only the rows that changed.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            self.lists = Lists::default();
            self.rows.clear();
            cx.notify();
            return;
        };
        let now = artisan_api::now();
        let held = self.held.as_ref().map(|(id, state)| (id, *state));
        self.lists = list::lists(host.read(cx), self.open.as_ref(), held, &self.icons, now);
        let weak = cx.entity().downgrade();
        let mut rows = HashMap::with_capacity(self.rows.len());
        let history = self.lists.history.iter().flat_map(|group| &group.rows);
        for data in self.lists.inbox.iter().chain(history) {
            let row = if let Some(row) = self.rows.remove(&data.id) {
                row.update(cx, |row, cx| row.set(data.clone(), cx));
                row
            } else {
                let (data, theme, weak) = (data.clone(), self.theme, weak.clone());
                let hover = self.threads_hover.clone();
                cx.new(|_| ThreadRow::new(data, theme, weak, hover))
            };
            rows.insert(data.id.clone(), row);
        }
        self.rows = rows;
        self.mark_open_read(&host, cx);
        self.schedule_tick(now, cx);
        cx.notify();
    }

    /// Whatever opened the thread (a row, the palette, a link), new
    /// activity in it is read while it is on screen.
    fn mark_open_read(&mut self, host: &Entity<HostData>, cx: &mut Context<Self>) {
        let Some(open) = self.open.clone() else {
            return;
        };
        let Some(thread) = host.read(cx).threads.get(&open) else {
            return;
        };
        let seen = (open.clone(), thread.activity_seq);
        if !thread.is_unread() || self.marked.as_ref() == Some(&seen) {
            return;
        }
        let read_seq = thread.activity_seq;
        self.marked = Some(seen);
        let client = host.read(cx).client.clone();
        self.call(cx, format!("mark thread {open} read"), async move {
            client
                .update_thread(
                    &open,
                    &UpdateThread {
                        read_seq: Some(read_seq),
                        ..UpdateThread::default()
                    },
                )
                .await
        });
    }

    /// Repaints when the soonest age label changes; nothing polls.
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
        let delay = std::time::Duration::from_millis(u64::try_from(at - now).unwrap_or(0));
        let task = cx.spawn(async move |sidebar, cx| {
            cx.background_executor().timer(delay).await;
            sidebar
                .update(cx, |sidebar, cx| {
                    sidebar.tick = None;
                    sidebar.refresh(cx);
                })
                .ok();
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
        cx.spawn(async move |sidebar, cx| {
            if let Err(error) = call.await {
                tracing::error!("couldn't {what}: {error:#}");
                sidebar
                    .update(cx, |sidebar, cx| {
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
        let Some(activity) = self
            .host
            .as_ref()
            .and_then(|host| host.read(cx).threads.get(&thread))
            .map(|thread| thread.activity_seq)
        else {
            return;
        };
        // Don't mark it read again just because it is open.
        self.marked = Some((thread.clone(), activity));
        if self.held.as_ref().is_some_and(|(id, _)| id == &thread) {
            self.held = None;
        }
        let update = UpdateThread {
            read_seq: Some(activity.saturating_sub(1)),
            ..UpdateThread::default()
        };
        self.update_thread(thread, "mark unread", update, cx);
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
        window.focus(&editor.focus_handle(cx), cx);
        let submitted = cx.subscribe(
            &editor,
            move |sidebar, editor, event: &TextEditorEvent, cx| {
                if *event != TextEditorEvent::Submitted {
                    return;
                }
                let title = editor.read(cx).text().trim().to_owned();
                sidebar.rename = None;
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
        let mut settled: serde_json::Map<String, serde_json::Value> = list::settled(data)
            .into_iter()
            .filter(|(id, _)| data.threads.contains_key(id))
            .map(|(id, seq)| (id.0, seq.into()))
            .collect();
        settled.insert(thread.0.clone(), activity.into());
        let client = data.client.clone();
        if self.held.as_ref().is_some_and(|(id, _)| id == thread) {
            self.held = None;
        }
        let read = UpdateThread {
            read_seq: Some(activity),
            ..UpdateThread::default()
        };
        self.update_thread(thread.clone(), "settle", read, cx);
        self.call(cx, format!("settle thread {thread}"), async move {
            client
                .set_setting(list::SETTLED_KEY, &serde_json::Value::Object(settled))
                .await
        });
    }

    /// Adds a project: the system folder dialog where the host has one,
    /// else a dialog browsing the host's folders.
    pub fn add_project(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        let (client, host_id) = (host.read(cx).client.clone(), host.read(cx).id.clone());
        let kind = self
            .core
            .read(cx)
            .hosts()
            .find(|(config, _)| config.id == host_id)
            .map(|(config, _)| config.kind.clone())
            .unwrap_or_default();
        if let Some(chosen) = folder_dialog::choose(&kind, cx) {
            cx.spawn(async move |sidebar, cx| {
                let created = async {
                    let Some(path) = chosen.await.context("choosing a folder")? else {
                        return Ok(None);
                    };
                    client
                        .create_project(&CreateProject {
                            id: ProjectId::new(),
                            path: path.clone(),
                            name: None,
                        })
                        .await
                        .with_context(|| format!("adding {path}"))
                        .map(Some)
                }
                .await;
                sidebar
                    .update(cx, |sidebar, cx| match created {
                        Ok(Some(project)) => sidebar.navigate(
                            Screen::NewThread {
                                host: host_id,
                                project: Some(project.id),
                            },
                            cx,
                        ),
                        Ok(None) => {}
                        Err(error) => {
                            tracing::error!("couldn't add a project: {error:#}");
                            sidebar.error = Some(format!("Couldn't add a project: {error:#}"));
                            cx.notify();
                        }
                    })
                    .ok();
            })
            .detach();
            return;
        }
        let theme = self.theme;
        let picker = cx.new(|cx| FolderPicker::new(client, theme, cx));
        let subscription =
            cx.subscribe(
                &picker,
                move |sidebar, _, event: &PickerEvent, cx| match event {
                    PickerEvent::Added(project) => {
                        sidebar.picker = None;
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

    fn switch_host(&mut self, host: HostId, cx: &mut Context<Self>) {
        self.close_profile(cx);
        self.navigate(
            Screen::NewThread {
                host,
                project: None,
            },
            cx,
        );
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

// Rendering.
impl Sidebar {
    fn desktop(&self) -> DesktopTheme {
        let mut theme = DesktopTheme::neutral_dark();
        theme.secondary = self.theme.colors.muted_foreground.to_paint();
        theme
    }

    /// A 16 px slot holding a 15 px glyph in the muted colour.
    fn nav_glyph(&self, icon: AssetId) -> Div {
        div()
            .w(px(16.0))
            .h(px(16.0))
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .text_color(self.desktop().secondary)
            .child(asset_glyph(icon).size(px(15.0)))
    }

    /// The pointer's hover fill: the lit gradient with its highlight.
    fn hoverable(&self, element: Stateful<Div>) -> Stateful<Div> {
        let theme = self.theme;
        let shadow = hover_highlight_shadow(&theme);
        element.hover(move |style| style.bg(hover_fill_gradient(theme)).shadow(shadow.clone()))
    }

    /// One navigation row: glyph and label; the open destination's label
    /// reads in the link colour, never a fill. The surface's pill flies to
    /// it under the pointer.
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
            .debug_selector(move || id.to_owned())
            .relative()
            .w_full()
            .h(px(34.0))
            .flex()
            .flex_none()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .child(surface.row_probe(hover_id))
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                hover.hovered(hover_id, *hovered);
                cx.notify();
            }))
            .child(self.nav_glyph(icon))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(if current {
                        self.theme.colors.banner_info.to_paint()
                    } else {
                        self.desktop().foreground
                    })
                    .child(label),
            )
    }

    fn rows(&self, rows: &[RowData]) -> Div {
        let mut list = div().w_full().flex_none().flex().flex_col().gap(px(2.0));
        for data in rows {
            if let Some(row) = self.rows.get(&data.id) {
                // The probe sits outside the cached row: a cached view
                // replays its paint, so a probe inside it would keep the
                // position the row had when it last rendered.
                list = list.child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .child(
                            row.clone()
                                .cached(StyleRefinement::default().w_full().h(px(ROW_HEIGHT))),
                        )
                        .child(
                            self.threads_hover
                                .row_probe(format!("sidebar-thread-{}", data.id)),
                        ),
                );
            }
        }
        list
    }

    /// Asks Forge once per project for its icon file and its repository.
    fn load_projects(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        let projects: Vec<ProjectId> = host.read(cx).projects.keys().cloned().collect();
        host.update(cx, |host, cx| {
            for project in &projects {
                host.load_repository(project, false, cx);
            }
        });
        let client = host.read(cx).client.clone();
        for project in projects {
            if self.icons.contains_key(&project) {
                continue;
            }
            self.icons.insert(project.clone(), None);
            let client = client.clone();
            cx.spawn(async move |sidebar, cx| {
                let icon = match client.project_icon(&project).await {
                    Ok(Some(blob)) => client.blob(&blob.hash).await.map(Some),
                    Ok(None) => Ok(None),
                    Err(error) => Err(error),
                };
                match icon {
                    Ok(Some(bytes)) => {
                        let image = Arc::new(Image::from_bytes(ImageFormat::Webp, bytes));
                        let _ = sidebar.update(cx, |sidebar, cx| {
                            sidebar.icons.insert(project, Some(image));
                            sidebar.refresh(cx);
                        });
                    }
                    Ok(None) => {}
                    Err(error) => tracing::debug!(%project, "no project icon: {error:#}"),
                }
            })
            .detach();
        }
    }

    /// History's heading row, shaped like the navigation rows: glyph,
    /// label, a glass count and a chevron that turns down when open.
    fn render_history(&self, cx: &mut Context<Self>) -> Div {
        let muted = self.theme.colors.muted_foreground.to_paint();
        let count = self.lists.history_len();
        let header = self
            .nav_row(
                "sidebar-history",
                HISTORY_HOVER_ID,
                &self.threads_hover,
                AssetId::TABLER_ARCHIVE,
                "History",
                false,
                cx,
            )
            .child(div().flex_1())
            .child(glass_badge(&self.theme, count.to_string()))
            .child(
                asset_glyph(if self.history_open {
                    AssetId::TABLER_CHEVRON_DOWN
                } else {
                    AssetId::TABLER_CHEVRON_RIGHT
                })
                .size(px(12.0))
                .flex_none()
                .text_color(muted),
            )
            .on_click(cx.listener(|sidebar, _, _, cx| {
                sidebar.history_open = !sidebar.history_open;
                cx.notify();
            }));
        div()
            .w_full()
            .flex()
            .flex_none()
            .flex_col()
            .gap(px(2.0))
            .child(header)
            .when(self.history_open, |section| {
                section.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(self.theme.spacing.steps(3.0))
                        .children(self.lists.history.iter().map(|group| {
                            let label = group.age.label().map(|label| {
                                let id = format!("sidebar-history-{}", group.age.id());
                                // The thread-age heading the inspector's
                                // section labels share.
                                div()
                                    .debug_selector(move || id)
                                    .w_full()
                                    .px(px(8.0))
                                    .pb(self.theme.spacing.steps(1.0))
                                    .text_size(self.theme.typography.label_text)
                                    .text_color(muted)
                                    .truncate()
                                    .child(label)
                            });
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .children(label)
                                .child(self.rows(&group.rows))
                        })),
                )
            })
    }

    /// The thread row menu: glass, a row per action, Archive set apart
    /// in red.
    #[allow(clippy::too_many_lines)]
    fn render_menu(&self, menu: &Menu, cx: &mut Context<Self>) -> AnyElement {
        let desktop = self.desktop();
        let MenuTarget::Thread(thread) = menu.target.clone();
        let data = self
            .host
            .as_ref()
            .and_then(|host| host.read(cx).threads.get(&thread).cloned());
        let unread = data
            .as_ref()
            .is_some_and(|thread| thread.is_unread() || thread.activity_seq == 0);
        let settleable = self
            .lists
            .inbox
            .iter()
            .any(|row| row.id == thread && row.settleable);
        let red = self.theme.colors.destructive;
        // A lighter, quieter red than the label, so the icon reads as muted
        // beside it.
        let red_glyph = Oklch::new((red.l + 0.08).min(1.0), red.c * 0.7, red.h).to_paint();
        let item = |id: &'static str, icon: AssetId, label: &'static str, destructive: bool| {
            let (glyph, text) = if destructive {
                (red_glyph, red.to_paint())
            } else {
                (desktop.secondary, desktop.foreground)
            };
            self.hoverable(
                div()
                    .id(id)
                    .debug_selector(move || id.to_owned())
                    .relative()
                    .px(px(12.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(RadiusTokens::value(RadiusStep::Xl))
                    .cursor_pointer()
                    .child(
                        div()
                            .size(px(16.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .flex_shrink_0()
                            .child(asset_glyph(icon).size(px(15.0)).text_color(glyph)),
                    )
                    .child(
                        div()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .whitespace_nowrap()
                            .text_color(text)
                            .child(label),
                    ),
            )
        };
        let mut items: Vec<AnyElement> = Vec::new();
        if settleable {
            let id = thread.clone();
            items.push(
                item(
                    "sidebar-menu-settle",
                    AssetId::TABLER_CIRCLE_CHECK,
                    "Settle Thread",
                    false,
                )
                .on_click(cx.listener(move |sidebar, _, _, cx| {
                    sidebar.menu = None;
                    sidebar.settle(&id, cx);
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        let rename = thread.clone();
        items.push(
            item(
                "sidebar-menu-rename",
                AssetId::TABLER_PENCIL,
                "Rename Thread",
                false,
            )
            .on_click(cx.listener(move |sidebar, _, window, cx| {
                sidebar.menu = None;
                sidebar.start_rename(rename.clone(), window, cx);
            }))
            .into_any_element(),
        );
        if !unread {
            let id = thread.clone();
            items.push(
                item(
                    "sidebar-menu-unread",
                    AssetId::TABLER_BELL,
                    "Mark as Unread",
                    false,
                )
                .on_click(cx.listener(move |sidebar, _, _, cx| {
                    sidebar.menu = None;
                    sidebar.mark_unread(id.clone(), cx);
                    cx.notify();
                }))
                .into_any_element(),
            );
        }
        items.push(
            div()
                .h(px(1.0))
                .my(px(4.0))
                .bg(self.theme.colors.border.with_alpha(0.5).to_paint())
                .into_any_element(),
        );
        items.push(
            item(
                "sidebar-menu-archive",
                AssetId::TABLER_ARCHIVE,
                "Archive Thread",
                true,
            )
            .on_click(cx.listener(move |sidebar, _, _, cx| {
                sidebar.menu = None;
                let update = UpdateThread {
                    archived: Some(true),
                    ..UpdateThread::default()
                };
                sidebar.update_thread(thread.clone(), "archive", update, cx);
                cx.notify();
            }))
            .into_any_element(),
        );
        let radius = RadiusTokens::value(RadiusStep::X2l);
        deferred(
            anchored()
                .position(menu.position)
                .snap_to_window_with_margin(px(8.0))
                .child(
                    div()
                        .id("sidebar-menu")
                        .w(px(208.0))
                        .p(px(4.0))
                        .rounded(radius)
                        .border_1()
                        .border_color(desktop.line)
                        .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
                        .bg(glass_foreground_base(&self.theme))
                        .shadow(glass_card_shadows())
                        .relative()
                        .flex()
                        .flex_col()
                        .block_mouse_except_scroll()
                        .child(glass_material_layer(GlassStrength::Quiet, radius))
                        .child(glass_highlight_layer(GlassStrength::Quiet, radius))
                        .children(items)
                        .on_mouse_down_out(cx.listener(|sidebar, _, _, cx| {
                            sidebar.menu = None;
                            cx.notify();
                        })),
                ),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn avatar_for(&mut self, seed: &str) -> Arc<Image> {
        self.avatars
            .entry(seed.to_owned())
            .or_insert_with(|| avatar::avatar_image(seed))
            .clone()
    }

    /// Opens the account menu: it grows from its bottom-left corner.
    fn open_profile(&mut self, cx: &mut Context<Self>) {
        self.profile_open = true;
        self.machines_open = false;
        self.chevron_toggled = Some(Instant::now());
        self.profile_hover.clear();
        self.usage.clear();
        let generation = self.profile_motion.borrow_mut().begin_open();
        self.settle_profile(generation, true, cx);
        cx.notify();
    }

    /// Closes the account menu; it stays painted for its exit.
    fn close_profile(&mut self, cx: &mut Context<Self>) {
        if !self.profile_open {
            return;
        }
        self.profile_open = false;
        self.machines_open = false;
        self.chevron_toggled = Some(Instant::now());
        self.profile_hover.clear();
        self.usage.clear();
        let generation = self.profile_motion.borrow_mut().begin_close();
        self.settle_profile(generation, false, cx);
        cx.notify();
    }

    fn settle_profile(&mut self, generation: u64, open: bool, cx: &mut Context<Self>) {
        let settle = move |sidebar: &mut Self| {
            let mut motion = sidebar.profile_motion.borrow_mut();
            if open {
                motion.finish_open(generation)
            } else {
                motion.finish_close(generation)
            }
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
        self.profile_settle = Some(cx.spawn(async move |sidebar, cx| {
            cx.background_executor().timer(duration).await;
            sidebar
                .update(cx, |sidebar, cx| {
                    if settle(sidebar) {
                        sidebar.profile_settle = None;
                        cx.notify();
                    }
                })
                .ok();
        }));
    }

    /// Asks Forge to read `engine`'s account and usage again.
    pub(crate) fn refresh_usage(&mut self, engine: EngineId, cx: &mut Context<Self>) {
        let Some(host) = self.host.clone() else {
            return;
        };
        if !self.usage.refreshing.insert(engine) {
            return;
        }
        let client = host.read(cx).client.clone();
        cx.notify();
        cx.spawn(async move |sidebar, cx| {
            let result = client
                .engine_action(engine, "refresh", serde_json::json!({}))
                .await;
            sidebar
                .update(cx, |sidebar, cx| {
                    sidebar.usage.refreshing.remove(&engine);
                    if let Err(error) = result {
                        tracing::error!(%engine, "couldn't refresh the usage: {error:#}");
                        sidebar.error = Some(format!(
                            "Couldn't refresh {}'s usage: {error:#}",
                            usage::engine_name(engine)
                        ));
                    }
                    cx.notify();
                })
                .ok();
        })
        .detach();
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

    /// The account footer: the host's avatar, the account Forge runs as and
    /// the host's name, and a chevron that turns as the menu opens.
    #[allow(clippy::too_many_lines)]
    fn render_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let desktop = self.desktop();
        let (seed, name, machine, status) = match &self.host {
            Some(host) => {
                let host = host.read(cx);
                // Master seeds a host's avatar with its certificate's SHA-256,
                // which is the fingerprint the Editor pins.
                let seed = self
                    .core
                    .read(cx)
                    .hosts()
                    .find(|(config, _)| config.id == host.id)
                    .map_or_else(
                        || host.id.0.clone(),
                        |(config, _)| config.fingerprint.clone(),
                    );
                (
                    seed,
                    host.account
                        .as_ref()
                        .map_or_else(|| host.name.clone(), |account| account.user.clone()),
                    host.name.clone(),
                    Self::status_text(&host.status),
                )
            }
            None => (
                "no-host".to_owned(),
                "Artisan".to_owned(),
                "No host".to_owned(),
                None,
            ),
        };
        let image = self.avatar_for(&seed);
        let turn = self.chevron_turn(window, cx);
        let hover = self.nav_hover.clone();
        let trigger = div()
            .id("artisan-desktop-profile-trigger")
            .debug_selector(|| "artisan-desktop-profile-trigger".to_owned())
            .relative()
            .cursor_pointer()
            .rounded(px(8.0))
            .w_full()
            .h(px(44.0))
            .p(px(5.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(self.nav_hover.row_probe(PROFILE_HOVER_ID))
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                hover.hovered(PROFILE_HOVER_ID, *hovered);
                cx.notify();
            }))
            .child(
                div()
                    .size(px(32.0))
                    .flex_shrink_0()
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .child(avatar::avatar(image.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.0))
                            .line_height(px(16.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(desktop.foreground)
                            .child(name.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(10.0))
                            .line_height(px(12.0))
                            .text_color(match status {
                                Some(_) => self.theme.colors.banner_warning.to_paint(),
                                None => desktop.secondary,
                            })
                            .child(status.clone().unwrap_or_else(|| machine.clone().into())),
                    ),
            )
            .child(
                div()
                    .w(px(16.0))
                    .h(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .child(
                        gpui::svg()
                            .path(AssetId::TABLER_CHEVRON_UP.as_str())
                            .size(px(15.0))
                            .text_color(desktop.secondary)
                            .with_transformation(gpui::Transformation::rotate(gpui::radians(
                                turn * std::f32::consts::PI,
                            ))),
                    ),
            )
            .on_click(cx.listener(|sidebar, _, _, cx| {
                cx.stop_propagation();
                if sidebar.profile_open {
                    sidebar.close_profile(cx);
                } else {
                    sidebar.open_profile(cx);
                }
            }));
        let origin = Rc::clone(&self.profile_origin);
        let root = div()
            .relative()
            .w_full()
            .child(
                div()
                    .child(trigger)
                    .on_children_prepainted(move |bounds, _, _| {
                        if let Some(bounds) = bounds.first() {
                            origin.set(*bounds);
                        }
                    }),
            );
        if !self.profile_motion.borrow().painted() {
            return root;
        }
        let menu = self.render_profile_menu(image, name, machine, status, window, cx);
        let machines = self.machines_open.then(|| self.render_machines(window, cx));
        root.child(menu).children(machines)
    }

    /// Glides a wheel tick over the thread list, as master's did; trackpad
    /// deltas and reduced motion move it directly. A list with nothing to
    /// scroll leaves the wheel to what is around it.
    fn threads_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = self.threads_scroll.clone();
        let max = f32::from(handle.max_offset().y).max(0.0);
        if max <= 0.0 {
            return;
        }
        cx.stop_propagation();
        let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        let offset = handle.offset();
        let current = f32::from(offset.y);
        if event.delta.precise() || cx.reduce_motion() {
            self.threads_glide.cancel();
            handle.set_offset(point(offset.x, px((current + delta).clamp(-max, 0.0))));
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
        cx.on_next_frame(window, |sidebar, window, cx| {
            sidebar.threads_glide_scheduled = false;
            let handle = sidebar.threads_scroll.clone();
            let offset = handle.offset();
            let max = f32::from(handle.max_offset().y).max(0.0);
            if let Some(next) = sidebar.threads_glide.step(f32::from(offset.y), max) {
                handle.set_offset(point(offset.x, px(next)));
                cx.notify();
            }
            sidebar.schedule_threads_glide(window, cx);
        });
    }

    fn open_machines(&mut self) {
        if !self.machines_open {
            self.machines_open = true;
            self.machines_hover.clear();
        }
    }

    /// The paired hosts, on a card of the same glass beside the account
    /// menu, its top at the host row (master's machine submenu).
    #[allow(clippy::too_many_lines)]
    fn render_machines(&mut self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let desktop = DesktopTheme {
            foreground: self.theme.colors.foreground.to_paint(),
            secondary: self.theme.colors.muted_foreground.to_paint(),
            ..self.desktop()
        };
        let active = self.host.as_ref().map(|host| host.read(cx).id.clone());
        let hosts: Vec<_> = self
            .core
            .read(cx)
            .hosts()
            .map(|(config, entity)| {
                let status = Self::status_text(&entity.read(cx).status);
                (config.clone(), status)
            })
            .collect();
        let hosts: Vec<_> = hosts
            .into_iter()
            .map(|(config, status)| {
                let image = self.avatar_for(&config.fingerprint);
                (config, status, image)
            })
            .collect();
        let bounds = machines_bounds(
            self.machines_trigger.get(),
            self.profile_panel.get(),
            window.viewport_size(),
            hosts.len(),
        );
        let hover = self.machines_hover.clone();
        let row = |id: SharedString, hover_id: String, cx: &mut Context<Self>| {
            let hover = hover.clone();
            div()
                .id(id)
                .relative()
                .px(px(8.0))
                .py(px(8.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .rounded(RadiusTokens::value(RadiusStep::Xl))
                .cursor_pointer()
                .text_color(desktop.foreground)
                .child(self.machines_hover.row_probe(hover_id.clone()))
                .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                    if *hovered {
                        hover.hovered(&hover_id, true);
                        cx.notify();
                    }
                }))
        };
        let mut rows = div()
            .id("machine-submenu-rows")
            .relative()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (config, status, image) in hosts {
            let id = config.id.clone();
            let current = active.as_ref() == Some(&id);
            rows = rows.child(
                row(
                    format!("machine-option-{}", id.0).into(),
                    format!("host-{}", id.0),
                    cx,
                )
                .child(
                    div()
                        .size(px(32.0))
                        .flex_shrink_0()
                        .rounded(px(8.0))
                        .overflow_hidden()
                        .child(avatar::avatar(image)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .truncate()
                                .text_size(px(14.0))
                                .line_height(px(20.0))
                                .child(config.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.0))
                                .line_height(px(16.0))
                                .text_color(match status {
                                    Some(_) => self.theme.colors.banner_warning.to_paint(),
                                    None => desktop.secondary,
                                })
                                .child(status.unwrap_or_else(|| config.address.clone().into())),
                        ),
                )
                .when(current, |row| {
                    row.child(self.nav_glyph(AssetId::TABLER_CHECK))
                })
                .on_click(cx.listener(move |sidebar, _, _, cx| {
                    cx.stop_propagation();
                    sidebar.switch_host(id.clone(), cx);
                })),
            );
        }
        rows = rows
            .child(
                div().flex_shrink_0().h(px(1.0)).my(px(4.0)).bg(self
                    .theme
                    .colors
                    .foreground
                    .with_alpha(0.2)
                    .to_paint()),
            )
            .child(
                row("sidebar-manage-hosts".into(), "manage-hosts".to_owned(), cx)
                    .child(self.nav_glyph(AssetId::TABLER_SETTINGS))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .child("Manage hosts…"),
                    )
                    .on_click(cx.listener(|sidebar, _, _, cx| {
                        sidebar.close_profile(cx);
                        sidebar.navigate(Screen::Settings(SettingsPage::Hosts), cx);
                    })),
            );
        let radius = RadiusTokens::value(RadiusStep::X2l);
        let clear_hover = self.machines_hover.clone();
        let panel = div()
            .id("machine-dropdown")
            .debug_selector(|| "machine-dropdown".to_owned())
            .w(bounds.size.width)
            .max_h(bounds.size.height)
            .p(px(4.0))
            .rounded(radius)
            .border_1()
            .border_color(desktop.line)
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&self.theme))
            .shadow(glass_card_shadows())
            .relative()
            .flex()
            .flex_col()
            .block_mouse_except_scroll()
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius))
            .child(bounds_probe(Rc::clone(&self.machines_panel)))
            .child(
                div()
                    .id("machine-hover-surface")
                    .relative()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .child(self.machines_hover.surface_probe())
                    .child(self.machines_hover.pill(
                        "machines",
                        &self.theme,
                        RadiusTokens::value(RadiusStep::Xl),
                        cx.reduce_motion(),
                    ))
                    .child(rows),
            )
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                if !*hovered {
                    clear_hover.clear();
                    cx.notify();
                }
            }))
            .on_click(cx.listener(|_, _, _, cx| cx.stop_propagation()))
            .on_mouse_down_out(cx.listener(|sidebar, event: &gpui::MouseDownEvent, _, cx| {
                // The account menu closes itself on a click outside both.
                if sidebar.profile_panel.get().contains(&event.position) {
                    return;
                }
                sidebar.machines_open = false;
                cx.notify();
            }));
        deferred(anchored().position(bounds.origin).child(panel))
            .with_priority(2)
            .into_any_element()
    }

    /// The menu above the footer (`desktop_profile`): the host, which
    /// lists the paired hosts, then each engine's usage, then Settings and
    /// Usage, on the quiet glass. It takes the footer's width and grows
    /// from its bottom-left corner.
    #[allow(clippy::too_many_lines)]
    fn render_profile_menu(
        &self,
        image: Arc<Image>,
        name: String,
        machine: String,
        status: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        // The dropdown reads in the shared text tokens.
        let desktop = DesktopTheme {
            foreground: self.theme.colors.foreground.to_paint(),
            secondary: self.theme.colors.muted_foreground.to_paint(),
            ..self.desktop()
        };
        let radius = RadiusTokens::value(RadiusStep::X2l);
        let border = self.theme.colors.border;
        let separator = border.with_alpha(border.a * 0.5).to_paint();
        let hover = self.profile_hover.clone();
        let action =
            |id: &'static str, icon: AssetId, label: SharedString, cx: &mut Context<Self>| {
                let hover = hover.clone();
                div()
                    .id(id)
                    .debug_selector(move || id.to_owned())
                    .relative()
                    .px(px(12.0))
                    .py(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .rounded(RadiusTokens::value(RadiusStep::Xl))
                    .cursor_pointer()
                    .child(self.profile_hover.row_probe(id))
                    .child(self.nav_glyph(icon))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(desktop.foreground)
                            .child(label),
                    )
                    .on_hover(cx.listener(move |sidebar, hovered: &bool, _, cx| {
                        if *hovered {
                            sidebar.machines_open = false;
                            hover.hovered(id, true);
                            cx.notify();
                        }
                    }))
            };
        let host_hover = self.profile_hover.clone();
        let host_row = div()
            .id("artisan-desktop-profile-machine")
            .debug_selector(|| "artisan-desktop-profile-machine".to_owned())
            .relative()
            .p(px(8.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .rounded(RadiusTokens::value(RadiusStep::Xl))
            .cursor_pointer()
            .child(self.profile_hover.row_probe("profile-host"))
            .child(bounds_probe(Rc::clone(&self.machines_trigger)))
            // Hovering the host opens its submenu beside the menu.
            .on_hover(cx.listener(move |sidebar, hovered: &bool, _, cx| {
                if *hovered {
                    sidebar.open_machines();
                    host_hover.hovered("profile-host", true);
                    cx.notify();
                }
            }))
            .child(
                div()
                    .size(px(32.0))
                    .flex_shrink_0()
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .child(avatar::avatar(image)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(desktop.foreground)
                            .child(name),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .text_color(desktop.secondary)
                            .child(status.unwrap_or_else(|| machine.into())),
                    ),
            )
            .child(
                // Nudged toward the submenu while it is open.
                gpui::svg()
                    .path(AssetId::TABLER_CHEVRON_RIGHT.as_str())
                    .size(px(15.0))
                    .flex_none()
                    .text_color(desktop.secondary)
                    .with_transformation(gpui::Transformation::translate(point(
                        px(if self.machines_open { 2.0 } else { 0.0 }),
                        px(0.0),
                    ))),
            )
            .on_click(cx.listener(|sidebar, _, _, cx| {
                cx.stop_propagation();
                sidebar.open_machines();
                cx.notify();
            }));
        let engines = self
            .host
            .as_ref()
            .map(|host| usage::shown(host.read(cx).engines.values()))
            .unwrap_or_default();
        let show_usage = !engines.is_empty();
        let clear_hover = self.profile_hover.clone();
        let width = self.profile_origin.get().size.width.max(px(256.0));
        let mut panel = div()
            .id("artisan-desktop-profile-menu")
            .debug_selector(|| "artisan-desktop-profile-menu".to_owned())
            .w(width)
            .rounded(radius)
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&self.theme))
            .border_1()
            .border_color(desktop.line)
            .shadow(glass_card_shadows())
            .relative()
            .flex()
            .flex_col()
            .block_mouse_except_scroll()
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius))
            .child(self.usage.menu_probe())
            .child(bounds_probe(Rc::clone(&self.profile_panel)))
            .child(self.profile_hover.surface_probe())
            .child(self.profile_hover.pill(
                "profile",
                &self.theme,
                RadiusTokens::value(RadiusStep::Xl),
                cx.reduce_motion(),
            ))
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                if !*hovered {
                    clear_hover.clear();
                    cx.notify();
                }
            }))
            .child(
                div()
                    .debug_selector(|| "artisan-desktop-profile-header".to_owned())
                    .p(px(4.0))
                    .child(host_row),
            );
        panel = panel.child(div().h(px(1.0)).bg(separator).my(px(4.0)));
        if show_usage {
            let viewport = f32::from(window.viewport_size().height);
            let top = f32::from(self.profile_origin.get().top()).min(viewport);
            let max_height =
                (top - 4.0 - PROFILE_MENU_VIEWPORT_MARGIN - PROFILE_MENU_CHROME).max(0.0);
            panel = panel
                .child(
                    div()
                        .id("artisan-profile-usage-scroll")
                        .debug_selector(|| "artisan-profile-usage-scroll".to_owned())
                        .min_h(px(0.0))
                        .max_h(px(max_height))
                        .overflow_y_scroll()
                        .child(usage::section(
                            self,
                            &engines,
                            &self.theme,
                            desktop,
                            separator,
                            cx,
                        )),
                )
                .child(div().h(px(1.0)).bg(separator).my(px(4.0)));
        }
        let mut actions = div()
            .id("artisan-desktop-profile-actions-hover-surface")
            .debug_selector(|| "artisan-desktop-profile-actions-hover-surface".to_owned())
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .p(px(4.0))
            .child(
                action(
                    "artisan-desktop-profile-action-0",
                    AssetId::TABLER_SETTINGS,
                    "Settings".into(),
                    cx,
                )
                .on_click(cx.listener(|sidebar, _, _, cx| {
                    sidebar.close_profile(cx);
                    sidebar.navigate(Screen::Settings(SettingsPage::Models), cx);
                })),
            );
        if show_usage {
            let refresh: Vec<EngineId> = engines.iter().map(|(engine, _)| *engine).collect();
            actions = actions.child(
                action(
                    "artisan-desktop-profile-action-1",
                    AssetId::TABLER_LIST_DETAILS,
                    "Usage".into(),
                    cx,
                )
                // Usage reads every engine again and keeps the menu open.
                .on_click(cx.listener(move |sidebar, _, _, cx| {
                    cx.stop_propagation();
                    for engine in &refresh {
                        sidebar.refresh_usage(*engine, cx);
                    }
                })),
            );
        }
        panel = panel.child(actions);
        if let Some(tooltip) = usage::tooltip(self, &engines, &self.theme, desktop) {
            panel = panel.child(tooltip);
        }
        let origin = Rc::clone(&self.profile_origin);
        let machines = Rc::clone(&self.machines_panel);
        let panel = panel.on_mouse_down_out(cx.listener(
            move |sidebar, event: &gpui::MouseDownEvent, _, cx| {
                let in_machines = sidebar.machines_open && machines.get().contains(&event.position);
                if !origin.get().contains(&event.position) && !in_machines {
                    sidebar.close_profile(cx);
                }
            },
        ));
        deferred(
            anchored()
                .anchor(gpui::Anchor::BottomLeft)
                .position(self.profile_origin.get().origin)
                .offset(point(px(0.0), px(-4.0)))
                .child(animate_menu(
                    panel,
                    &self.profile_motion,
                    "profile",
                    // Above the footer, aligned to its left edge: it grows
                    // from that bottom-left corner.
                    MenuEntrance::Scale {
                        origin: point(0.0, 1.0),
                    },
                    PROFILE_MENU_OPEN,
                    PROFILE_MENU_CLOSE,
                )),
        )
        .with_priority(1)
        .into_any_element()
    }

    fn status_text(status: &ConnectionStatus) -> Option<SharedString> {
        match status {
            ConnectionStatus::Live => None,
            ConnectionStatus::Connecting => Some("Connecting…".into()),
            ConnectionStatus::Offline { reason, .. } => Some(format!("Offline: {reason}").into()),
        }
    }

    /// A modal dialog over the whole window.
    fn dialog(
        &self,
        id: &'static str,
        title: &'static str,
        content: impl IntoElement,
        window: &Window,
        cx: &mut Context<Self>,
        close: fn(&mut Self),
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let weak: WeakEntity<Self> = cx.entity().downgrade();
        let dialog = Dialog::new(
            id,
            self.dialog_focus.clone(),
            self.theme,
            MotionPolicy::Full,
            true,
            title,
            content,
        )
        .debug_selector(id)
        .on_dismiss(move |_, _, cx| {
            weak.update(cx, |sidebar, cx| {
                close(sidebar);
                cx.notify();
            })
            .ok();
        });
        deferred(
            anchored()
                .position(point(px(0.0), px(0.0)))
                .child(div().w(viewport.width).h(viewport.height).child(dialog)),
        )
        .with_priority(2)
        .into_any_element()
    }

    fn render_rename(
        &self,
        rename: &Rename,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let field = div()
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(8.0))
            .border_1()
            .border_color(self.theme.colors.ring.to_paint())
            .child(rename.editor.clone());
        self.dialog(
            "sidebar-rename",
            "Rename thread",
            field,
            window,
            cx,
            |sidebar| sidebar.rename = None,
        )
    }
}

impl Render for Sidebar {
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.load_projects(cx);
        let colors = &self.theme.colors;
        let screen = self.core.read(cx).screen().clone();
        let new_thread = self
            .nav_row(
                "sidebar-new-thread",
                NEW_THREAD_HOVER_ID,
                &self.nav_hover,
                AssetId::TABLER_EDIT,
                "New thread",
                false,
                cx,
            )
            .on_click(cx.listener(|sidebar, _, _, cx| sidebar.new_thread(None, cx)));
        let marketplace = self
            .nav_row(
                "sidebar-marketplace",
                MARKETPLACE_HOVER_ID,
                &self.nav_hover,
                AssetId::TABLER_SHOPPING_BAG,
                "Marketplace",
                matches!(screen, Screen::Marketplace),
                cx,
            )
            .on_click(cx.listener(|sidebar, _, _, cx| {
                sidebar.navigate(Screen::Marketplace, cx);
            }));
        let reduce_motion = cx.reduce_motion();
        let (nav_hover, threads_hover) = (self.nav_hover.clone(), self.threads_hover.clone());
        // The rows sit in one content block, the thread pill's surface; it
        // grows to the viewport when the rows are shorter, which keeps
        // History at the bottom.
        let content = div()
            .id("artisan-sidebar-threads-content")
            .relative()
            .w_full()
            .flex_grow(1.0)
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap(self.theme.spacing.steps(3.0))
            // Inside the scroll container, so it eases a wheel tick before
            // the container scrolls by it.
            .on_scroll_wheel(cx.listener(Self::threads_wheel))
            .child(self.threads_hover.surface_probe())
            .child(
                self.threads_hover
                    .pill("sidebar-threads", &self.theme, px(6.0), reduce_motion),
            )
            .when(!self.lists.inbox.is_empty(), |threads| {
                threads.child(self.rows(&self.lists.inbox))
            })
            .when(!self.lists.history.is_empty(), |threads| {
                // History keeps to the foot of the list.
                threads
                    .child(div().flex_1().min_h(px(0.0)))
                    .child(self.render_history(cx))
            });
        let threads = div()
            .id("sidebar-scroll")
            .debug_selector(|| "sidebar-threads".to_owned())
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.threads_scroll)
            .flex()
            .flex_col()
            .on_hover(cx.listener(move |_, hovered: &bool, _, cx| {
                if *hovered {
                    nav_hover.hide();
                } else {
                    threads_hover.clear();
                }
                cx.notify();
            }))
            .child(content);
        let error = self.error.clone().map(|error| {
            div()
                .id("sidebar-error")
                .px(px(8.0))
                .text_size(self.theme.typography.label_text)
                .text_color(colors.destructive.to_paint())
                .cursor_pointer()
                .child(error)
                .on_click(cx.listener(|sidebar, _, _, cx| {
                    sidebar.error = None;
                    cx.notify();
                }))
        });
        let line = self.desktop().line;
        let profile = self.render_profile(window, cx);
        let menu = self.menu.as_ref().map(|menu| self.render_menu(menu, cx));
        let rename = self
            .rename
            .as_ref()
            .map(|rename| self.render_rename(rename, window, cx));
        let picker = self.picker.as_ref().map(|(picker, _)| {
            self.dialog(
                "sidebar-add-project",
                "Add a project",
                picker.clone(),
                window,
                cx,
                |sidebar| sidebar.picker = None,
            )
        });
        div()
            .id("sidebar")
            .size_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(10.0))
            .child(
                // One hover surface for the column: the same pill travels
                // among New thread, Marketplace and the footer.
                div()
                    .id("artisan-workspace-navigation-hover-surface")
                    .relative()
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .on_hover(cx.listener(|sidebar, hovered: &bool, _, cx| {
                        if !*hovered {
                            sidebar.nav_hover.clear();
                            cx.notify();
                        }
                    }))
                    .child(self.nav_hover.surface_probe())
                    .child(
                        self.nav_hover
                            .pill("sidebar", &self.theme, px(6.0), reduce_motion),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(new_thread)
                            .child(marketplace),
                    )
                    .child(threads)
                    .children(error)
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(10.0))
                            .child(
                                div()
                                    .h(px(1.0))
                                    .mx(px(-10.0))
                                    .bg(line)
                                    .debug_selector(|| "artisan-sidebar-footer-divider".to_owned()),
                            )
                            .child(profile),
                    ),
            )
            .children(menu)
            .children(rename)
            .children(picker)
    }
}

/// Records an element's window bounds; place it inside the element.
fn bounds_probe(bounds: Rc<Cell<gpui::Bounds<Pixels>>>) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |measured, (), window, cx| {
            if bounds.replace(measured) != measured {
                window.defer(cx, |window, _| window.refresh());
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// The host submenu beside the account menu, its top at the host row; it
/// flips to the menu's left when the right has no room.
fn machines_bounds(
    trigger: gpui::Bounds<Pixels>,
    parent: gpui::Bounds<Pixels>,
    viewport: gpui::Size<Pixels>,
    hosts: usize,
) -> gpui::Bounds<Pixels> {
    let width = px(300.0).min((viewport.width - px(16.0)).max(px(0.0)));
    // Each host row, the separator and Manage hosts, inside the padding.
    let rows = f32::from(u16::try_from(hosts).unwrap_or(u16::MAX));
    let height = px(48.0 * rows + 9.0 + 36.0 + 10.0)
        .min(px(400.0))
        .min((viewport.height - px(16.0)).max(px(0.0)));
    let right = parent.right() + px(6.0);
    let left = if right + width <= viewport.width - px(8.0) {
        right
    } else {
        parent.left() - width - px(6.0)
    };
    gpui::Bounds::new(
        point(
            left.max(px(8.0))
                .min((viewport.width - width - px(8.0)).max(px(0.0))),
            trigger
                .top()
                .max(px(8.0))
                .min((viewport.height - height - px(8.0)).max(px(0.0))),
        ),
        gpui::size(width, height),
    )
}
