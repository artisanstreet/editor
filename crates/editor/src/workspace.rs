//! The window's layout. Forwards nothing but focus and visibility; every
//! section talks to `Core` and Forge itself.

use crate::context_meter::{ContextMeter, ContextReading};
use crate::home::Home;
use crate::notifications::Notifications;
use crate::palette::{Palette, PaletteEvent};
use crate::{GoBack, NewThread, OpenPalette, OpenSettings, ToggleInspector, ToggleSidebar};
use editor_conversation::{Conversation, ConversationEvent};
use editor_core::{ConnectionStatus, Core, CoreEvent, HostData, HostEvent, Screen, SettingsPage};
use editor_inspector::Inspector;
use editor_marketplace::Marketplace;
use editor_settings::NOTIFICATIONS_KEY;
use editor_settings::{BuildInfo, Settings};
use editor_sidebar::Sidebar;
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::glass::glass_badge;
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme, ColorTokens, ProseTypography, SurfaceStep};
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, FocusHandle, FontWeight,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Window,
    WindowControlArea, div, px,
};

pub const KEY_CONTEXT: &str = "Workspace";

/// Titlebar height.
const TITLEBAR_HEIGHT: f32 = 48.0;
/// Horizontal inset of the titlebar's content section, matching the
/// wordmark section's padding.
const TITLEBAR_CONTENT_INSET: f32 = 14.0;
/// The left sidebar's width, and the thread inspector's, so the two ruled
/// columns read as a pair.
pub const COLUMN_WIDTH: f32 = 327.0;
/// The narrowest either ruled column shrinks to before it hides.
const COLUMN_MIN_WIDTH: f32 = 240.0;
/// The chat's comfortable minimum: the 768 px reading column plus 48 px of
/// rail clearance each side. Columns shrink, then hide, before it does.
pub const CHAT_MIN_WIDTH: f32 = 768.0 + 2.0 * 48.0;
/// A caption button's width.
use editor_ui::window_controls::CONTROL_WIDTH;
/// Gap between the context ring and the rule or controls to its right.
const CONTEXT_METER_GAP: f32 = 16.0;
/// The ring's slot width.
const CONTEXT_METER_WIDTH: f32 = 24.0;
/// The shell's one face: true black behind every column.
const SHELL_BLACK: u32 = 0x0000_0000;

/// The sidebar's width for a window, `None` while it is hidden: the chat
/// keeps its minimum first and the sidebar takes what remains, up to
/// [`COLUMN_WIDTH`].
fn sidebar_width(window_width: f32) -> Option<f32> {
    let room = window_width - CHAT_MIN_WIDTH;
    (room >= COLUMN_MIN_WIDTH).then(|| room.min(COLUMN_WIDTH))
}

/// The inspector's width for the content width beside the sidebar, `None`
/// while it does not fit.
fn inspector_width(content_width: f32) -> Option<f32> {
    let room = content_width - CHAT_MIN_WIDTH;
    (room >= COLUMN_MIN_WIDTH).then(|| room.min(COLUMN_WIDTH))
}

/// The build badge: `D` and the commit's first seven characters.
fn build_badge() -> Option<String> {
    let commit = env!("EDITOR_COMMIT");
    (!commit.is_empty()).then(|| {
        format!(
            "D{}",
            commit.get(..7).unwrap_or(commit).to_ascii_uppercase()
        )
    })
}

pub struct Workspace {
    core: Entity<Core>,
    sidebar: Entity<Sidebar>,
    conversation: Entity<Conversation>,
    inspector: Entity<Inspector>,
    settings: Option<Entity<Settings>>,
    build: BuildInfo,
    marketplace: Option<Entity<Marketplace>>,
    home: Option<Entity<Home>>,
    palette: Option<Entity<Palette>>,
    resource_preview: Option<Entity<crate::resource_preview::ResourcePreview>>,
    copy_menu: Option<editor_ui::markdown_renderer::ShowCopyMenu>,
    show_sidebar: bool,
    show_inspector: bool,
    _repository: Entity<crate::repository_context::RepositoryContext>,
    /// Shown above everything until dismissed.
    failure: Option<String>,
    focus: FocusHandle,
    notifications: Notifications,
    host_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(
        core: Entity<Core>,
        failure: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // A paired Editor starts on a new thread rather than the welcome.
        let start = {
            let core = core.read(cx);
            match core.screen() {
                Screen::Home => core.active_host().map(|host| host.read(cx).id.clone()),
                _ => None,
            }
        };
        if let Some(host) = start {
            core.update(cx, |core, cx| {
                core.navigate(
                    Screen::NewThread {
                        host,
                        project: None,
                    },
                    cx,
                );
            });
        }

        let sidebar = cx.new(|cx| Sidebar::new(core.clone(), window, cx));
        let conversation = cx.new(|cx| Conversation::new(core.clone(), window, cx));
        let inspector = cx.new(|cx| Inspector::new(core.clone(), conversation.clone(), window, cx));
        let build = BuildInfo {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: env!("EDITOR_COMMIT").to_owned(),
            profile: if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            }
            .to_owned(),
            target: env!("EDITOR_TARGET").to_owned(),
        };
        editor_settings::frame_rate::initialize(core.read(cx), window);
        let subscriptions = vec![
            cx.subscribe_in(&core, window, |this, _, event, window, cx| {
                if matches!(event, CoreEvent::HostsChanged) {
                    this.follow_hosts(window, cx);
                }
                if matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged) {
                    this.mount_screen(window, cx);
                }
                cx.notify();
            }),
            // The composer's "New project" row opens the folder picker the
            // sidebar owns.
            cx.subscribe(&conversation, |this, _, event: &ConversationEvent, cx| {
                match event {
                    ConversationEvent::NewProject => this.sidebar.update(cx, Sidebar::add_project),
                    // The title shows the open subagent.
                    ConversationEvent::AgentChanged => cx.notify(),
                    // An open agent's context meter follows its messages.
                    ConversationEvent::ItemsChanged => {
                        if this.conversation.read(cx).agent_context().is_some() {
                            cx.notify();
                        }
                    }
                }
            }),
        ];
        let repository =
            cx.new(|cx| crate::repository_context::RepositoryContext::new(core.clone(), cx));
        let mut workspace = Self {
            core,
            sidebar,
            conversation,
            inspector,
            settings: None,
            build,
            marketplace: None,
            home: None,
            palette: None,
            resource_preview: None,
            copy_menu: None,
            show_sidebar: true,
            show_inspector: true,
            _repository: repository,
            failure,
            focus: cx.focus_handle(),
            notifications: Notifications::default(),
            host_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
        };
        workspace.mount_screen(window, cx);
        workspace.follow_hosts(window, cx);
        window.focus(&workspace.focus, cx);
        workspace
    }

    fn mount_screen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.core.read(cx).screen() {
            Screen::Settings(_) if self.settings.is_none() => {
                self.settings =
                    Some(cx.new(|cx| {
                        Settings::new(self.core.clone(), self.build.clone(), window, cx)
                    }));
            }
            Screen::Marketplace if self.marketplace.is_none() => {
                self.marketplace = Some(cx.new(|cx| Marketplace::new(self.core.clone(), cx)));
            }
            Screen::Home if self.home.is_none() => {
                self.home = Some(cx.new(|cx| Home::new(self.core.clone(), cx)));
            }
            _ => {}
        }
    }

    /// Re-renders the connection notice when a host's status moves, and
    /// posts notifications for threads that finished or ask for an answer.
    fn follow_hosts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hosts: Vec<_> = self
            .core
            .read(cx)
            .hosts()
            .map(|(_, host)| host.clone())
            .collect();
        self.notifications.retain_hosts(
            &hosts
                .iter()
                .map(|host| host.read(cx).id.clone())
                .collect::<Vec<_>>(),
        );
        for host in &hosts {
            self.notifications.reset(host.read(cx));
        }
        self.host_subscriptions = hosts
            .into_iter()
            .map(|host| {
                cx.subscribe_in(&host, window, |this, host, event, window, cx| match event {
                    HostEvent::ThreadChanged { thread_id } => {
                        this.notify_changes(host, thread_id, window, cx);
                        if this
                            .open_thread(cx)
                            .is_some_and(|open| open.thread == *thread_id && open.host == *host)
                        {
                            cx.notify();
                        }
                    }
                    HostEvent::Reset => {
                        this.notifications.reset(host.read(cx));
                        cx.notify();
                    }
                    HostEvent::Changed => {
                        this.notifications.sync_connection(host.read(cx));
                        cx.notify();
                    }
                    _ => {}
                })
            })
            .collect();
    }

    fn notify_changes(
        &mut self,
        host: &Entity<HostData>,
        thread: &artisan_api::ThreadId,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let core = self.core.read(cx);
        let enabled = core
            .settings
            .get(NOTIFICATIONS_KEY)
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let host = host.read(cx);
        let watching = match core.screen() {
            Screen::Thread { host: open, thread }
                if window.is_window_active() && open == &host.id =>
            {
                Some(thread)
            }
            _ => None,
        };
        let posted = self.notifications.observe(host, thread, watching);
        if enabled && let Some(notification) = posted {
            cx.show_system_notification(notification);
        }
    }

    fn open_reference(
        &mut self,
        reference: &editor_ui::markdown_renderer::OpenReference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let client = reference.scope.as_ref().and_then(|scope| {
            self.core
                .read(cx)
                .host(&editor_core::HostId(scope.host.clone()))
                .map(|host| host.read(cx).client.clone())
        });
        editor_ui::overlay::begin("resource-preview", window, cx);
        let preview = cx
            .new(|cx| crate::resource_preview::ResourcePreview::new(reference.clone(), client, cx));
        cx.subscribe(
            &preview,
            |this, _, _: &crate::resource_preview::Dismissed, cx| {
                this.resource_preview = None;
                cx.notify();
            },
        )
        .detach();
        self.resource_preview = Some(preview);
        cx.notify();
    }

    fn show_copy_menu(
        &mut self,
        menu: &editor_ui::markdown_renderer::ShowCopyMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        editor_ui::overlay::begin("copy-menu", window, cx);
        self.copy_menu = Some(menu.clone());
        cx.notify();
    }

    fn render_copy_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.copy_menu.as_ref()?;
        let theme = *cx.theme();
        let rows = menu.entries.iter().enumerate().map(|(index, entry)| {
            let text = entry.text.clone();
            editor_ui::button::Button::text(("copy-value", index), theme, entry.label.clone())
                .debug_selector(format!("copy-value-{index}"))
                .variant(editor_ui::button::ButtonVariant::Ghost)
                .on_activate(cx.listener(move |this, _, window, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_string()));
                    this.copy_menu = None;
                    editor_ui::overlay::dismiss("copy-menu", window, cx);
                    cx.notify();
                }))
        });
        let this = cx.entity().downgrade();
        Some(
            editor_ui::popover::Popover::new(
                "copy-menu",
                "Copy value",
                menu.position,
                div().flex().flex_col().gap(px(4.0)).children(rows),
                move |_, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.copy_menu = None;
                        cx.notify();
                    });
                },
            )
            .into_any_element(),
        )
    }

    fn open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return;
        }
        let palette = cx.new(|cx| Palette::new(self.core.clone(), window, cx));
        cx.subscribe_in(&palette, window, |this, _, event, window, cx| match event {
            PaletteEvent::Dismissed => {
                this.palette = None;
                editor_ui::overlay::dismiss("command-palette", window, cx);
                cx.notify();
            }
        })
        .detach();
        self.palette = Some(palette);
        cx.notify();
    }

    fn navigate(&self, screen: Screen, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| core.navigate(screen, cx));
    }

    fn new_thread(&mut self, _: &NewThread, _: &mut Window, cx: &mut Context<Self>) {
        let host = self
            .core
            .read(cx)
            .active_host()
            .map(|host| host.read(cx).id.clone());
        match host {
            Some(host) => self.navigate(
                Screen::NewThread {
                    host,
                    project: None,
                },
                cx,
            ),
            None => self.navigate(Screen::Home, cx),
        }
    }

    /// The open thread's host, project and title.
    fn open_thread(&self, cx: &Context<Self>) -> Option<OpenThread> {
        let core = self.core.read(cx);
        let Screen::Thread { host, thread } = core.screen() else {
            return None;
        };
        let host = core.host(host)?.clone();
        let data = host.read(cx);
        let record = data.threads.get(thread);
        Some(OpenThread {
            title: record
                .and_then(|thread| thread.title.clone())
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "Thread".to_owned()),
            project: record.map(|thread| thread.project_id.clone()),
            thread: thread.clone(),
            host,
        })
    }

    /// The titlebar's leading section above the sidebar: the `Artisan
    /// Editor` wordmark, which goes home, and the build badge.
    fn brand(theme: &ArtisanTheme, desktop: &ColorTokens, cx: &Context<Self>) -> Div {
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .child(
                div()
                    .id("artisan-brand-home")
                    .focusable().tab_index(0).role(gpui::Role::Button).aria_label("Artisan Editor home")
                    .debug_selector(|| "artisan-brand-home".to_owned())
                    .block_mouse_except_scroll()
                    .line_height(px(28.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        let host = this
                            .core
                            .read(cx)
                            .active_host()
                            .map(|host| host.read(cx).id.clone());
                        match host {
                            Some(host) => this.navigate(
                                Screen::NewThread {
                                    host,
                                    project: None,
                                },
                                cx,
                            ),
                            None => this.navigate(Screen::Home, cx),
                        }
                    }))
                    .flex_shrink_0()
                    .text_size(px(26.0))
                    .font_family("Test Founders Grotesk")
                    .font_weight(FontWeight::SEMIBOLD)
                    // -0.025em tracking matches the Founders specimen.
                    .letter_spacing(px(-0.65))
                    .text_color(desktop.workspace_foreground.to_paint())
                    .child("Artisan Editor"),
            )
            .children(
                build_badge()
                    .map(|badge| glass_badge(theme, badge).debug_selector("artisan-brand-build")),
            )
    }

    /// The workspace header on a thread: the repository's host mark and
    /// `owner/repository` link (else the project folder), a darker `/`, and
    /// the thread's title, which alone truncates.
    fn header(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> Option<AnyElement> {
        let open = self.open_thread(cx)?;
        let host = open.host.read(cx);
        let project = open
            .project
            .as_ref()
            .and_then(|project| host.projects.get(project));
        let repository = open
            .project
            .as_ref()
            .and_then(|project| host.repository(project))
            .filter(|repository| repository.web_url.is_some());
        let muted = theme.colors.muted_foreground.to_paint();
        let mut cluster = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .text_color(muted)
            .debug_selector(|| "artisan-desktop-titlebar-header".to_owned());
        if let Some(project) = project {
            if let Some(repository) = repository {
                let web_url = repository.web_url.clone().unwrap_or_default();
                let mark = repository_mark(repository.host.as_deref());
                let mut glyph = asset_glyph(mark.asset).size(px(14.0));
                if mark.monochrome {
                    glyph = glyph.text_color(theme.colors.foreground.to_paint());
                }
                let destination = SharedString::from(web_url.clone());
                cluster = cluster.child(div().flex_shrink_0().child(glyph)).child(
                    div()
                        .id("artisan-desktop-titlebar-repository-link")
                        .focusable()
                        .tab_index(0)
                        .role(gpui::Role::Button)
                        .aria_label(format!("Open {}", qualified_label(&web_url)))
                        .cursor_pointer()
                        .flex_shrink_0()
                        .text_color(theme.colors.banner_info.to_paint())
                        .on_click(move |_, _, cx| cx.open_url(destination.as_ref()))
                        .child(qualified_label(&web_url)),
                );
            } else {
                cluster = cluster.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .flex_shrink_0()
                        .text_color(theme.colors.foreground.to_paint())
                        .child(
                            asset_glyph(AssetId::TABLER_FOLDER)
                                .size(px(14.0))
                                .text_color(muted)
                                .flex_shrink_0(),
                        )
                        .child(project.name.clone()),
                );
            }
            cluster = cluster.child(
                div()
                    .flex_shrink_0()
                    // Two ramp steps below the muted line around it.
                    .text_color(SurfaceStep::S600.oklch().to_paint())
                    .child("/"),
            );
        }
        // A subagent's view adds its name (and the agents above it) after
        // the thread's title; the title and each name above lead back.
        let trail = self.conversation.read(cx).agent_trail();
        let separator = || {
            div()
                .flex_shrink_0()
                .text_color(SurfaceStep::S600.oklch().to_paint())
                .child("/")
        };
        let conversation = self.conversation.clone();
        let mut title = div()
            .id("artisan-desktop-titlebar-title")
            .min_w(px(0.0))
            .truncate()
            .debug_selector(|| "artisan-desktop-titlebar-title".to_owned())
            .child(open.title);
        if trail.is_empty() {
            title = title.flex_1();
        } else {
            title = title
                .focusable()
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label("Open parent thread")
                .flex_shrink(1.0)
                .cursor_pointer()
                .hover(|style| style.text_color(theme.colors.foreground.to_paint()))
                .on_click(move |_, _, cx| {
                    conversation.update(cx, |conversation, cx| conversation.open_agent(None, cx));
                });
        }
        cluster = cluster.child(title);
        let last = trail.len().saturating_sub(1);
        for (index, (agent, name)) in trail.into_iter().enumerate() {
            let mut crumb = div()
                .id(SharedString::from(format!(
                    "artisan-desktop-titlebar-agent-{agent}"
                )))
                .flex_shrink_0()
                .child(name);
            if index == last {
                crumb = crumb.text_color(theme.colors.foreground.to_paint());
            } else {
                let conversation = self.conversation.clone();
                crumb = crumb
                    .focusable()
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .aria_label("Open parent agent")
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.colors.foreground.to_paint()))
                    .on_click(move |_, _, cx| {
                        let agent = agent.clone();
                        conversation.update(cx, |conversation, cx| {
                            conversation.open_agent(Some(agent), cx)
                        });
                    });
            }
            cluster = cluster.child(separator()).child(crumb);
        }
        Some(cluster.into_any_element())
    }

    /// The meter's reading: the open thread's run, or the open agent's own
    /// context while its view is shown (an agent's model can differ).
    fn context_reading(&self, cx: &Context<Self>) -> Option<ContextReading> {
        let open = self.open_thread(cx)?;
        let host = open.host.read(cx);
        let run = host.runs.get(&open.thread)?;
        let (used, model) = match self.conversation.read(cx).agent_context() {
            Some(agent) => {
                let (used, model) = agent?;
                (used, model.or_else(|| run.model.clone())?)
            }
            None => (run.usage.context_used?, run.model.clone()?),
        };
        let known = host
            .engines
            .get(&run.engine)
            .and_then(|engine| engine.models.iter().find(|known| known.id == model));
        let window = if run.model.as_deref() == Some(model.as_str()) {
            run.usage.context_window
        } else {
            None
        }
        .or_else(|| known.and_then(|known| known.context_window))?;
        let name = known.map_or_else(|| model.clone(), |known| known.name.clone());
        ContextReading::new(name, used, window)
    }

    /// The titlebar across the window: the brand above the sidebar, the
    /// workspace header, the context ring, the caption buttons, and the
    /// sidebar's and inspector's rules continued through it.
    #[allow(clippy::too_many_lines)]
    fn titlebar(
        &self,
        sidebar: Option<Pixels>,
        inspector: Option<Pixels>,
        rule: Pixels,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = *cx.theme();
        let desktop = cx.theme().colors;
        let reading = self.context_reading(cx);
        let controls_width = px(CONTROL_WIDTH * 3.0);
        let meter_right = inspector.unwrap_or(px(0.0)).max(controls_width) + px(CONTEXT_METER_GAP);
        let content_inset = if reading.is_some() {
            (meter_right + px(CONTEXT_METER_WIDTH + 24.0) - controls_width).max(px(24.0))
        } else {
            px(24.0)
        };
        let brand = sidebar.map(|width| {
            div()
                .w(width)
                .h_full()
                .flex_shrink_0()
                .flex()
                .items_center()
                .px(px(14.0))
                .overflow_hidden()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .h_full()
                        .flex()
                        .items_center()
                        .window_control_area(WindowControlArea::Drag)
                        .child(Self::brand(&theme, &desktop, cx)),
                )
        });
        let content = div()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .flex()
            .items_center()
            .overflow_hidden()
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .h_full()
                    .flex()
                    .items_center()
                    .pl(px(TITLEBAR_CONTENT_INSET))
                    .pr(content_inset)
                    .overflow_hidden()
                    .children(self.header(&theme, cx))
                    // Siblings of the header, so its link keeps its click.
                    .child(
                        div()
                            .flex_1()
                            .h_full()
                            .window_control_area(WindowControlArea::Drag),
                    ),
            );
        let controls =
            editor_ui::window_controls::WindowControls::new(theme, window.is_maximized());
        let meter = reading.map(|reading| {
            div()
                .absolute()
                .right(meter_right)
                .top(px(0.0))
                .h(px(TITLEBAR_HEIGHT))
                .flex()
                .items_center()
                .child(ContextMeter::new(reading, theme))
        });
        div()
            .relative()
            .w_full()
            .h(px(TITLEBAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(desktop.line.to_paint())
            .debug_selector(|| "artisan-desktop-titlebar".to_owned())
            .children(brand)
            .child(content)
            .child(controls)
            .children(sidebar.map(|width| {
                // The sidebar's right rule, continued through the bar.
                div()
                    .absolute()
                    .left(width - rule)
                    .top(px(0.0))
                    .w(rule)
                    .h(px(TITLEBAR_HEIGHT))
                    .bg(desktop.line.to_paint())
            }))
            .children(inspector.map(|width| {
                // The inspector's left rule, continued through the bar.
                div()
                    .absolute()
                    .right(width - rule)
                    .top(px(0.0))
                    .w(rule)
                    .h(px(TITLEBAR_HEIGHT))
                    .bg(desktop.line.to_paint())
                    .debug_selector(|| "artisan-desktop-inspector-titlebar-divider".to_owned())
            }))
            .children(meter)
            .into_any_element()
    }

    /// The active host's connection, when it isn't live.
    fn connection_notice(&self, desktop: &ColorTokens, cx: &Context<Self>) -> Option<AnyElement> {
        let host = self.core.read(cx).active_host()?.read(cx);
        let text = match &host.status {
            ConnectionStatus::Live => return None,
            ConnectionStatus::Connecting => format!("Connecting to {}…", host.name),
            ConnectionStatus::Offline {
                attempt, reason, ..
            } => {
                format!(
                    "{} is unreachable ({reason}). Retrying, attempt {attempt}.",
                    host.name
                )
            }
        };
        Some(notice(desktop, text, None))
    }
}

fn notice(desktop: &ColorTokens, text: String, dismiss: Option<AnyElement>) -> AnyElement {
    div()
        .w_full()
        .px(px(14.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .bg(desktop.field.to_paint())
        .border_b_1()
        .border_color(desktop.line.to_paint())
        .text_sm()
        .text_color(desktop.workspace_foreground.to_paint())
        .child(div().flex_1().child(text))
        .children(dismiss)
        .into_any_element()
}

impl Render for Workspace {
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let desktop = theme.colors;
        let screen = self.core.read(cx).screen().clone();
        let rule = px(1.0 / window.scale_factor().max(0.5));
        let window_width = f32::from(window.viewport_size().width);
        // Settings keeps the sidebar, as before.
        let sidebar_width = self
            .show_sidebar
            .then(|| sidebar_width(window_width))
            .flatten();
        let content_width = window_width - sidebar_width.unwrap_or(0.0);
        // The new-thread screen paints the same column, empty, so its
        // composer sits where a thread's does.
        let inspector_width = (self.show_inspector
            && matches!(screen, Screen::Thread { .. } | Screen::NewThread { .. }))
        .then(|| inspector_width(content_width))
        .flatten();
        let center: AnyElement = match &screen {
            Screen::Home => self
                .home
                .as_ref()
                .expect("home mounted on navigation")
                .clone()
                .into_any_element(),
            Screen::Settings(_) => self
                .settings
                .as_ref()
                .expect("settings mounted on navigation")
                .clone()
                .into_any_element(),
            Screen::Marketplace => self
                .marketplace
                .as_ref()
                .expect("marketplace mounted on navigation")
                .clone()
                .into_any_element(),
            Screen::Thread { .. } | Screen::NewThread { .. } => {
                self.conversation.clone().into_any_element()
            }
        };
        let inspector = inspector_width.map(|width| {
            let column = div()
                .relative()
                .w(px(width))
                .h_full()
                .min_h(px(0.0))
                .flex_shrink_0()
                .bg(gpui::rgb(SHELL_BLACK))
                .debug_selector(|| "artisan-thread-screen-inspector".to_owned())
                .child(
                    div()
                        .absolute()
                        .left(px(0.0))
                        .top(px(0.0))
                        .bottom(px(0.0))
                        .w(rule)
                        .bg(desktop.line.to_paint()),
                );
            if matches!(screen, Screen::Thread { .. }) {
                // Cached: it notifies itself, so a frame drawn for anything
                // else reuses it.
                column.child(
                    self.inspector
                        .clone()
                        .cached(StyleRefinement::default().size_full()),
                )
            } else {
                column
            }
        });
        let sidebar = sidebar_width.map(|width| {
            div()
                .w(px(width))
                .h_full()
                .flex_shrink_0()
                .border_r_1()
                .border_color(desktop.line.to_paint())
                .debug_selector(|| "artisan-desktop-sidebar".to_owned())
                .child(self.sidebar.clone())
        });
        let failure = self.failure.clone().map(|text| {
            let dismiss = Button::text("dismiss-failure", *cx.theme(), "Dismiss")
                .variant(ButtonVariant::Ghost)
                .on_activate(cx.listener(|this, _, _, cx| {
                    this.failure = None;
                    cx.notify();
                }))
                .into_any_element();
            notice(&desktop, text, Some(dismiss))
        });
        let titlebar = self.titlebar(
            sidebar_width.map(px),
            inspector_width.map(px),
            rule,
            window,
            cx,
        );

        div()
            .id("workspace")
            .on_action(editor_ui::focus_navigation::focus_next)
            .on_action(editor_ui::focus_navigation::focus_previous)
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::open_reference))
            .on_action(cx.listener(Self::show_copy_menu))
            .on_action(cx.listener(Self::open_palette))
            .on_action(cx.listener(Self::new_thread))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.show_sidebar = !this.show_sidebar;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleInspector, _, cx| {
                this.show_inspector = !this.show_inspector;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| {
                this.core.update(cx, editor_core::Core::go_back);
            }))
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| {
                this.navigate(Screen::Settings(SettingsPage::Models), cx);
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            // One continuous true-black face; cards and popovers paint their
            // own surfaces above it.
            .bg(gpui::rgb(SHELL_BLACK))
            .text_color(desktop.workspace_foreground.to_paint())
            .font_family(cx.theme().typography.sans.family)
            .text_size(px(14.0))
            // One tracking for every text in the window, at its own size.
            .tracking(ProseTypography::TRACKING_EM)
            .debug_selector(|| "artisan-desktop-workspace".to_owned())
            .child(titlebar)
            .children(failure)
            .children(self.connection_notice(&desktop, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .flex()
                    .flex_row()
                    .children(sidebar)
                    .child(div().flex_1().min_w(px(0.0)).h_full().child(center))
                    .children(inspector),
            )
            // Cached like the inspector: the dialog notifies itself as its
            // job's output arrives.
            .child(
                self.inspector
                    .read(cx)
                    .terminal_dialog()
                    .clone()
                    .cached(StyleRefinement::default().absolute().inset_0()),
            )
            .children(self.palette.clone())
            .children(self.resource_preview.clone())
            .children(self.render_copy_menu(cx))
            // Above everything: an image opened from the composer or a
            // thread.
            .children(editor_ui::image_preview::overlay(&theme, window, cx))
    }
}

struct OpenThread {
    host: Entity<HostData>,
    thread: artisan_api::ThreadId,
    project: Option<artisan_api::ProjectId>,
    title: String,
}

/// A repository host's mark, and whether it is one colour (tinted to the
/// text) or keeps its own.
struct RepositoryMark {
    asset: AssetId,
    monochrome: bool,
}

fn repository_mark(host: Option<&str>) -> RepositoryMark {
    let (asset, monochrome) = match host {
        Some("github") => (AssetId::SVGL_GITHUB, true),
        Some("gitlab") => (AssetId::SVGL_GITLAB, false),
        Some("azure") => (AssetId::SVGL_MICROSOFT_AZURE, false),
        Some("bitbucket") => (AssetId::SIMPLE_ICONS_BITBUCKET, false),
        Some("codeberg") => (AssetId::SIMPLE_ICONS_CODEBERG, false),
        Some("gitea") => (AssetId::SIMPLE_ICONS_GITEA, false),
        Some("sourcehut") => (AssetId::SIMPLE_ICONS_SOURCEHUT, false),
        _ => (AssetId::SVGL_GIT, false),
    };
    RepositoryMark { asset, monochrome }
}

/// The innermost `owner/repository` of a web URL.
fn qualified_label(web_url: &str) -> String {
    let path = web_url
        .split_once("://")
        .map_or(web_url, |(_, rest)| rest)
        .trim_end_matches('/');
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match segments.as_slice() {
        [] => web_url.to_owned(),
        [host] => (*host).to_owned(),
        [_, name] => (*name).to_owned(),
        [.., owner, name] => format!("{owner}/{name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{inspector_width, qualified_label, sidebar_width};

    #[test]
    fn columns_shrink_then_hide_before_the_chat_gives_way() {
        assert_eq!(sidebar_width(1600.0), Some(327.0));
        assert_eq!(sidebar_width(1104.0), Some(240.0));
        assert_eq!(sidebar_width(1103.0), None);
        assert_eq!(inspector_width(1600.0 - 327.0), Some(327.0));
        assert_eq!(inspector_width(1440.0 - 327.0), Some(249.0));
        assert_eq!(inspector_width(1000.0), None);
    }

    #[test]
    fn the_repository_link_reads_owner_and_name() {
        assert_eq!(
            qualified_label("https://github.com/artisanstreet/editor"),
            "artisanstreet/editor"
        );
        assert_eq!(
            qualified_label("https://gitlab.com/group/sub/repo/"),
            "sub/repo"
        );
    }
}

#[cfg(all(test, feature = "capture"))]
mod rich_action_tests {
    use super::*;
    use editor_ui::markdown_renderer::{CopyEntry, OpenReference, ResourceScope, ShowCopyMenu};
    use gpui::{TestAppContext, point};

    #[gpui::test]
    fn workspace_dispatches_scoped_references_and_native_copy_menu(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let core = cx.new(|cx| {
            Core::new(
                editor_core::EditorFiles {
                    dir: dir.path().to_owned(),
                },
                cx,
            )
            .unwrap()
        });
        // Appearance needs no host or pairing discovery. This fixture cannot
        // launch, stop, install, pair with, or mutate the running application.
        core.update(cx, |core, cx| {
            core.navigate(Screen::Settings(editor_core::SettingsPage::Appearance), cx)
        });
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::new(core.clone(), None, window, cx));
        cx.run_until_parked();
        let reference = OpenReference {
            scope: Some(ResourceScope {
                host: "unavailable-owning-forge".into(),
                thread: artisan_api::ThreadId::new(),
            }),
            path: "/owning/forge/reference.rs".into(),
            line: Some(12),
        };
        cx.update(|window, cx| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("before reference".into()));
            assert!(window.is_action_available(&reference, cx));
            window.dispatch_action(Box::new(reference), cx);
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace.resource_preview.is_some(),
                "reference must open its Forge preview even when the owning Forge is unavailable"
            )
        });
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            "before reference"
        );
        let menu = ShowCopyMenu {
            position: point(px(100.0), px(100.0)),
            entries: vec![
                CopyEntry::primary("Copy original", "original value"),
                CopyEntry::alternative("Copy converted", "converted value"),
            ],
        };
        cx.update(|window, cx| {
            assert!(window.is_action_available(&menu, cx));
            window.dispatch_action(Box::new(menu), cx);
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(workspace.copy_menu.as_ref().unwrap().entries.len(), 2)
        });
        let entry = cx
            .debug_bounds("copy-value-1")
            .expect("shared menu alternative must be painted");
        cx.simulate_click(entry.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            cx.update(|_, cx| cx.read_from_clipboard().unwrap().text().unwrap()),
            "converted value"
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(
                workspace.copy_menu.is_none(),
                "native menu selection dismisses the menu"
            )
        });
    }
    #[derive(Clone, Debug, Default)]
    struct StalledSocket {
        bytes: usize,
        prefix: Vec<u8>,
        eof: bool,
        error: Option<String>,
    }
    /// Owns only loopback sockets. It never completes TLS or speaks HTTP.
    /// Nonblocking reads/accept and an owned stop flag bound teardown on panic.
    struct StalledTls {
        address: std::net::SocketAddr,
        sockets: std::sync::Arc<std::sync::Mutex<Vec<StalledSocket>>>,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl StalledTls {
        fn new() -> Self {
            use std::io::Read as _;
            use std::sync::{
                Arc, Mutex,
                atomic::{AtomicBool, Ordering},
            };
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let sockets = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let observed = sockets.clone();
            let stopped = stop.clone();
            let worker = std::thread::spawn(move || {
                let mut streams = Vec::new();
                while !stopped.load(Ordering::Acquire) {
                    loop {
                        if stopped.load(Ordering::Acquire) {
                            return;
                        }
                        match listener.accept() {
                            Ok((stream, _)) => {
                                stream.set_nonblocking(true).unwrap();
                                streams.push(stream);
                                observed.lock().unwrap().push(StalledSocket::default());
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => return,
                        }
                    }
                    for (index, stream) in streams.iter_mut().enumerate() {
                        let mut states = observed.lock().unwrap();
                        let state = &mut states[index];
                        if state.eof || state.error.is_some() {
                            continue;
                        }
                        let mut bytes = [0u8; 4096];
                        match stream.read(&mut bytes) {
                            Ok(0) => state.eof = true,
                            Ok(count) => {
                                state.bytes += count;
                                let take = count.min(3usize.saturating_sub(state.prefix.len()));
                                state.prefix.extend_from_slice(&bytes[..take]);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                            Err(error) => state.error = Some(error.to_string()),
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            });
            Self {
                address,
                sockets,
                stop,
                worker: Some(worker),
            }
        }
        fn sockets(&self) -> Vec<StalledSocket> {
            self.sockets.lock().unwrap().clone()
        }
    }
    impl Drop for StalledTls {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Release);
            if let Some(worker) = self.worker.take() {
                let _ = worker.join();
            }
        }
    }
    fn wait_for_preview_socket(
        cx: &mut gpui::VisualTestContext,
        server: &StalledTls,
        label: &str,
        ready: impl Fn(&[StalledSocket]) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            cx.run_until_parked();
            let sockets = server.sockets();
            if ready(&sockets) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{label}: {sockets:?}");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[gpui::test]
    fn closing_a_scoped_preview_releases_its_pending_request(cx: &mut TestAppContext) {
        use editor_core::{EditorFiles, HostConfig, HostId, HostKind, hosts::HostsFile};
        cx.executor().allow_parking();
        let server = StalledTls::new();
        let dir = tempfile::tempdir().unwrap();
        let files = EditorFiles {
            dir: dir.path().to_owned(),
        };
        let identity = files.identity().unwrap();
        let fingerprint =
            artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let host = HostId("isolated-preview-cancellation".into());
        files
            .save_hosts(&HostsFile {
                hosts: vec![HostConfig {
                    id: host.clone(),
                    name: "Isolated TLS stall".into(),
                    address: server.address.to_string(),
                    fingerprint,
                    kind: HostKind::Remote,
                }],
                last: None,
            })
            .unwrap();
        let core = cx.new(|cx| Core::new(files, cx).unwrap());
        // Appearance leaves only HostData's normal state fetch in flight.
        core.update(cx, |core, cx| {
            core.navigate(Screen::Settings(editor_core::SettingsPage::Appearance), cx)
        });
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::new(core.clone(), None, window, cx));
        wait_for_preview_socket(cx, &server, "HostData state TLS ClientHello", |sockets| {
            sockets.len() == 1 && sockets[0].bytes > 0 && sockets[0].prefix.starts_with(&[22, 3])
        });
        let reference = OpenReference {
            scope: Some(ResourceScope {
                host: host.0.clone(),
                thread: artisan_api::ThreadId::new(),
            }),
            path: "/isolated/source.rs".into(),
            line: Some(7),
        };
        let opened_at = std::time::Instant::now();
        cx.update(|window, cx| {
            assert!(window.is_action_available(&reference, cx));
            window.dispatch_action(Box::new(reference), cx);
        });
        wait_for_preview_socket(cx, &server, "ResourcePreview TLS ClientHello", |sockets| {
            sockets.len() == 2 && sockets[1].bytes > 0 && sockets[1].prefix.starts_with(&[22, 3])
        });
        let preview = workspace.read_with(cx, |workspace, _| {
            workspace.resource_preview.as_ref().unwrap().downgrade()
        });
        assert!(preview.upgrade().is_some());
        let closed_at = std::time::Instant::now();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx); // The actual Dialog installs Escape/focus.
            window.dispatch_event(
                gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke: gpui::Keystroke::parse("escape").unwrap(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                    keystroke: gpui::Keystroke::parse("escape").unwrap(),
                }),
                cx,
            );
        });
        wait_for_preview_socket(
            cx,
            &server,
            "closed preview request EOF before TLS timeout",
            |sockets| sockets.len() == 2 && sockets[1].eof,
        );
        assert!(closed_at.elapsed() < std::time::Duration::from_secs(2));
        assert!(
            opened_at.elapsed() < std::time::Duration::from_secs(10),
            "request cancellation must precede the production TLS connect timeout"
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.resource_preview.is_none())
        });
        assert!(
            preview.upgrade().is_none(),
            "Workspace close must release the pending preview owner"
        );
        let sockets = server.sockets();
        assert!(
            !sockets[0].eof && sockets[0].error.is_none(),
            "closing the preview must not cancel HostData's separate state fetch: {sockets:?}"
        );
        assert!(
            sockets[1].error.is_none(),
            "preview canceled with EOF: {sockets:?}"
        );
        // The fake peer has sent no TLS response, so its 10-second connect
        // timeout cannot be mistaken for successful parsing or normal EOF.
        // StalledTls::drop closes only these sockets and joins a polling worker.
    }
}
