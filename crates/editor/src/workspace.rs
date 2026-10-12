//! The window's layout. Forwards nothing but focus and visibility; every
//! section talks to `Core` and Forge itself.

use crate::context_meter::{ContextReading, context_card, context_ring};
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
use editor_ui::glass::glass_badge;
use editor_ui::motion::MotionDuration;
use editor_ui::picker_motion::{MenuEntrance, MenuMotion, animate_menu};
use editor_ui::theme::{ArtisanTheme, DesktopTheme, ProseTypography, SurfaceStep, ThemeMode};
use gpui::{
    AnyElement, AppContext as _, Context, Div, Entity, FocusHandle, FontWeight, Hsla,
    InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Render, SharedString,
    Stateful, StatefulInteractiveElement as _, StyleRefinement, Styled as _, Subscription, Window,
    WindowControlArea, div, px,
};
use gpui::{Bounds, Task};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

/// The context card's entrance and exit, the profile menu's own.
const CONTEXT_CARD_OPEN: Duration = MotionDuration::Fast.as_duration();
const CONTEXT_CARD_CLOSE: Duration = MotionDuration::Quick.as_duration();

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
const CHAT_MIN_WIDTH: f32 = 768.0 + 2.0 * 48.0;
/// A caption button's width.
const CONTROL_WIDTH: f32 = 46.0;
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
    settings: Entity<Settings>,
    marketplace: Entity<Marketplace>,
    home: Entity<Home>,
    palette: Option<Entity<Palette>>,
    show_sidebar: bool,
    show_inspector: bool,
    /// Whether the pointer is over the context ring, showing its card.
    context_card_open: bool,
    /// The card pops in and out on the profile menu's motion.
    context_motion: Rc<RefCell<MenuMotion>>,
    context_settle: Option<Task<()>>,
    /// The ring's bounds, which the card hangs from.
    context_bounds: Rc<Cell<Bounds<Pixels>>>,
    /// The project whose repository was last asked for.
    repository_for: Option<artisan_api::ProjectId>,
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
        let settings = cx.new(|cx| Settings::new(core.clone(), build, window, cx));
        editor_settings::frame_rate::initialize(core.read(cx), window);
        let home = cx.new(|cx| Home::new(core.clone(), cx));
        let marketplace = cx.new(|cx| Marketplace::new(core.clone(), cx));
        let subscriptions = vec![
            cx.subscribe_in(&core, window, |this, _, event, window, cx| {
                if matches!(event, CoreEvent::HostsChanged) {
                    this.follow_hosts(window, cx);
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
                    ConversationEvent::ItemsChanged => {}
                }
            }),
        ];
        let mut workspace = Self {
            core,
            sidebar,
            conversation,
            inspector,
            settings,
            marketplace,
            home,
            palette: None,
            show_sidebar: true,
            show_inspector: true,
            context_card_open: false,
            context_motion: Rc::default(),
            context_settle: None,
            context_bounds: Rc::default(),
            repository_for: None,
            failure,
            focus: cx.focus_handle(),
            notifications: Notifications::default(),
            host_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
        };
        workspace.follow_hosts(window, cx);
        window.focus(&workspace.focus, cx);
        workspace
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
        self.host_subscriptions = hosts
            .into_iter()
            .map(|host| {
                cx.subscribe_in(&host, window, |this, host, event, window, cx| {
                    if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                        this.notify_changes(host, window, cx);
                        cx.notify();
                    }
                })
            })
            .collect();
    }

    fn notify_changes(&mut self, host: &Entity<HostData>, window: &Window, cx: &mut Context<Self>) {
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
        let posted = self.notifications.observe(host, watching);
        if enabled {
            for notification in posted {
                cx.show_system_notification(notification);
            }
        }
    }

    fn open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            return;
        }
        let palette = cx.new(|cx| Palette::new(self.core.clone(), window, cx));
        cx.subscribe_in(&palette, window, |this, _, event, window, cx| match event {
            PaletteEvent::Dismissed => {
                this.palette = None;
                window.focus(&this.focus, cx);
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

    /// Asks the open thread's Forge for its project's branch and remote
    /// each time another project opens.
    fn follow_repository(&mut self, cx: &mut Context<Self>) {
        let open = self.open_thread(cx);
        let project = open.as_ref().and_then(|open| open.project.clone());
        if project == self.repository_for {
            return;
        }
        self.repository_for.clone_from(&project);
        if let (Some(open), Some(project)) = (open, project) {
            open.host
                .update(cx, |host, cx| host.load_repository(&project, true, cx));
        }
    }

    /// The titlebar's leading section above the sidebar: the `Artisan
    /// Editor` wordmark, which goes home, and the build badge.
    fn brand(theme: &ArtisanTheme, desktop: DesktopTheme, cx: &Context<Self>) -> Div {
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
                    .text_color(desktop.foreground)
                    .child("Artisan Editor"),
            )
            .children(build_badge().map(|badge| {
                glass_badge(theme, badge).debug_selector(|| "artisan-brand-build".to_owned())
            }))
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

    /// The open thread's context reading, from its latest run.
    /// Pops the context card in or out.
    fn set_context_card_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if open == self.context_card_open {
            return;
        }
        self.context_card_open = open;
        let generation = if open {
            self.context_motion.borrow_mut().begin_open()
        } else {
            self.context_motion.borrow_mut().begin_close()
        };
        let settle = move |this: &mut Self| {
            let mut motion = this.context_motion.borrow_mut();
            if open {
                motion.finish_open(generation)
            } else {
                motion.finish_close(generation)
            }
        };
        if cx.reduce_motion() {
            settle(self);
            self.context_settle = None;
        } else {
            let duration = if open {
                CONTEXT_CARD_OPEN
            } else {
                CONTEXT_CARD_CLOSE
            };
            self.context_settle = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(duration).await;
                this.update(cx, |this, cx| {
                    if settle(this) {
                        this.context_settle = None;
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        cx.notify();
    }

    fn context_reading(&self, cx: &Context<Self>) -> Option<ContextReading> {
        let open = self.open_thread(cx)?;
        let host = open.host.read(cx);
        let run = host.runs.get(&open.thread)?;
        let model = run.model.as_deref()?;
        let name = host
            .engines
            .get(&run.engine)
            .and_then(|engine| engine.models.iter().find(|known| known.id == model))
            .map_or_else(|| model.to_owned(), |known| known.name.clone());
        ContextReading::new(name, run.usage.context_used?, run.usage.context_window?)
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
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let desktop = DesktopTheme::neutral_dark();
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
                        .child(Self::brand(&theme, desktop, cx)),
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
        let controls = div()
            .flex()
            .h_full()
            .flex_shrink_0()
            .child(caption_button(
                &theme,
                WindowControlArea::Min,
                "artisan-desktop-titlebar-minimize",
            ))
            .child(maximize_button(desktop, window.is_maximized(), rule))
            .child(caption_button(
                &theme,
                WindowControlArea::Close,
                "artisan-desktop-titlebar-close",
            ));
        let meter = reading.map(|reading| {
            div()
                .absolute()
                .right(meter_right)
                .top(px(0.0))
                .h(px(TITLEBAR_HEIGHT))
                .flex()
                .items_center()
                .child({
                    let bounds = Rc::clone(&self.context_bounds);
                    div()
                        .child(context_ring(&reading, &theme).on_hover(cx.listener(
                            |this, hovered: &bool, _, cx| this.set_context_card_open(*hovered, cx),
                        )))
                        .on_children_prepainted(move |children, _, _| {
                            if let Some(first) = children.first() {
                                bounds.set(*first);
                            }
                        })
                })
                .children(self.context_motion.borrow().painted().then(|| {
                    let ring = self.context_bounds.get();
                    gpui::deferred(
                        gpui::anchored()
                            .anchor(gpui::Anchor::TopRight)
                            .position(ring.top_right())
                            .offset(gpui::point(px(0.0), ring.size.height + px(8.0)))
                            .child(animate_menu(
                                div()
                                    .id("artisan-context-card")
                                    .child(context_card(&reading, &theme)),
                                &self.context_motion,
                                "context",
                                // The card hangs below the ring, aligned to
                                // its right edge: it grows from that corner.
                                MenuEntrance::Scale {
                                    origin: gpui::point(1.0, 0.0),
                                },
                                CONTEXT_CARD_OPEN,
                                CONTEXT_CARD_CLOSE,
                            )),
                    )
                    .with_priority(1)
                }))
        });
        div()
            .relative()
            .w_full()
            .h(px(TITLEBAR_HEIGHT))
            .flex_shrink_0()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(desktop.line)
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
                    .bg(desktop.line)
            }))
            .children(inspector.map(|width| {
                // The inspector's left rule, continued through the bar.
                div()
                    .absolute()
                    .right(width - rule)
                    .top(px(0.0))
                    .w(rule)
                    .h(px(TITLEBAR_HEIGHT))
                    .bg(desktop.line)
                    .debug_selector(|| "artisan-desktop-inspector-titlebar-divider".to_owned())
            }))
            .children(meter)
            .into_any_element()
    }

    /// The active host's connection, when it isn't live.
    fn connection_notice(&self, theme: DesktopTheme, cx: &Context<Self>) -> Option<AnyElement> {
        let host = self.core.read(cx).active_host()?.read(cx);
        let text = match &host.status {
            ConnectionStatus::Live => return None,
            ConnectionStatus::Connecting => format!("Connecting to {}…", host.name),
            ConnectionStatus::Offline { attempt, reason } => {
                format!(
                    "{} is unreachable ({reason}). Retrying, attempt {attempt}.",
                    host.name
                )
            }
        };
        Some(notice(theme, text, None))
    }
}

fn notice(theme: DesktopTheme, text: String, dismiss: Option<AnyElement>) -> AnyElement {
    div()
        .w_full()
        .px(px(14.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .bg(theme.field)
        .border_b_1()
        .border_color(theme.line)
        .text_sm()
        .text_color(theme.foreground)
        .child(div().flex_1().child(text))
        .children(dismiss)
        .into_any_element()
}

impl Render for Workspace {
    #[allow(clippy::too_many_lines)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_repository(cx);
        let theme = DesktopTheme::neutral_dark();
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
            Screen::Home => self.home.clone().into_any_element(),
            Screen::Settings(_) => self.settings.clone().into_any_element(),
            Screen::Marketplace => self.marketplace.clone().into_any_element(),
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
                        .bg(theme.line),
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
                .border_color(theme.line)
                .debug_selector(|| "artisan-desktop-sidebar".to_owned())
                .child(self.sidebar.clone())
        });
        let failure = self.failure.clone().map(|text| {
            let dismiss = div()
                .id("dismiss-failure")
                .cursor_pointer()
                .text_color(theme.secondary)
                .child("Dismiss")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.failure = None;
                    cx.notify();
                }))
                .into_any_element();
            notice(theme, text, Some(dismiss))
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
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
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
            .text_color(theme.foreground)
            .font_family(ArtisanTheme::for_mode(ThemeMode::Dark).typography.sans.family)
            .text_size(px(14.0))
            // One tracking for every text in the window, at its own size.
            .tracking(ProseTypography::TRACKING_EM)
            .debug_selector(|| "artisan-desktop-workspace".to_owned())
            .child(titlebar)
            .children(failure)
            .children(self.connection_notice(theme, cx))
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
            // Above everything: an image opened from the composer or a
            // thread.
            .children(editor_ui::image_preview::overlay(
                &ArtisanTheme::for_mode(ThemeMode::Dark),
                window,
                cx,
            ))
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

/// One caption button. The platform handles mouse presses on caption
/// areas; the click handler is the keyboard path.
fn caption_button(
    theme: &ArtisanTheme,
    area: WindowControlArea,
    selector: &'static str,
) -> Stateful<Div> {
    let (hover_background, hover_foreground): (Hsla, Hsla) = match area {
        WindowControlArea::Close => (
            gpui::rgb_to_hsla(gpui::rgb(0x00e8_1123)),
            gpui::rgb_to_hsla(gpui::rgb(0x00ff_ffff)),
        ),
        _ => (
            theme.colors.muted.with_alpha(0.5).to_paint(),
            theme.colors.foreground.to_paint(),
        ),
    };
    let glyph = match area {
        WindowControlArea::Max => AssetId::TABLER_MAXIMIZE,
        WindowControlArea::Close => AssetId::TABLER_X,
        WindowControlArea::Min | WindowControlArea::Drag => AssetId::TABLER_MINUS,
    };
    div()
        .id(selector)
        .debug_selector(move || selector.to_owned())
        .w(px(CONTROL_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.colors.muted_foreground.to_paint())
        .hover(move |style| style.bg(hover_background).text_color(hover_foreground))
        .window_control_area(area)
        .on_click(move |event, window, _| {
            if !event.is_keyboard() {
                return;
            }
            match area {
                WindowControlArea::Min => window.minimize_window(),
                WindowControlArea::Max => window.zoom_window(),
                WindowControlArea::Close => window.remove_window(),
                WindowControlArea::Drag => {}
            }
        })
        .child(asset_glyph(glyph).size(px(16.0)))
}

/// The maximise / restore glyph follows the window's state: one square, or
/// two overlapping ones while maximised.
fn maximize_button(theme: DesktopTheme, maximized: bool, stroke: Pixels) -> Stateful<Div> {
    let mut glyph = div().relative().w(px(10.0)).h(px(10.0));
    if maximized {
        glyph = glyph
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(7.0))
                    .h(px(7.0))
                    .border_t(stroke)
                    .border_r(stroke)
                    .border_color(theme.foreground),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w(px(7.0))
                    .h(px(7.0))
                    .border(stroke)
                    .border_color(theme.foreground),
            );
    } else {
        glyph = glyph.border(stroke).border_color(theme.foreground);
    }
    div()
        .id("artisan-desktop-titlebar-maximize")
        .w(px(CONTROL_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .hover(move |style| style.bg(theme.selected))
        .window_control_area(WindowControlArea::Max)
        .on_click(|event, window, _| {
            if event.is_keyboard() {
                window.zoom_window();
            }
        })
        .child(glyph)
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
