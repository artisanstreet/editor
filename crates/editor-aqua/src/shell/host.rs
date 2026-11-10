use crate::core::{Core, CoreEvent, EditorFiles, HostEvent, HostId, Screen};
use artisan_api::ThreadId;
use crate::shell::entry::Entry;
use crate::ui::preferences::Preferences;
use crate::shell::workspace::{NotificationEffects, Workspace};
use aqua::AppCtx;
use aqua::images::ImageAssets;
use aqua::prelude::{AquaWindowId, WindowRequest};

pub struct Shell {
    pub titlebar: crate::shell::titlebar::Titlebar,
    pub core: Core,
    pub entry: Entry,
    pub preferences: Preferences,
    pub audio: editor_audio::Player,
    pub audio_assets: crate::ui::media::audio::AudioAssets,
    pub models: crate::ui::media::model::load::ModelAssets,
    pub meshes_supported: std::rc::Rc<std::cell::Cell<Option<bool>>>,
    pub meshes_changed: std::cell::Cell<bool>,
    pub dialogs: crate::ui::dialogs::DialogRoutes,
    pub live: crate::shell::live_throttle::LiveThrottle,
    images: ImageAssets,
}

pub struct ShellUpdates {
    pub notifications: Vec<NotificationEffects>,
    pub preferences_changed: bool,
    pub theme_transition: Option<bool>,
    pub settings_platform_effects: Vec<crate::shell::workspace::SettingsPlatformEffect>,
    pub changed: bool,
    pub live_only: bool,
}

impl Shell {
    pub fn new(files: EditorFiles, images: ImageAssets) -> (Self, ShellUpdates) {
        Self::opening(files, images, None)
    }

    pub fn opening(
        files: EditorFiles,
        images: ImageAssets,
        link: Option<Screen>,
    ) -> (Self, ShellUpdates) {
        aqua::native::startup_mark("core_init_begin");
        let (mut core, failure) = Core::init(files);
        aqua::native::startup_mark("core_init_done");
        if let Some(screen) = link {
            core.navigate(screen);
        }
        let failure =
            failure.map(|error| format!("Couldn't read this device's Artisan files: {error:#}"));
        Self::from_core(core, images, failure)
    }

    #[cfg(feature = "qualification")]
    pub fn for_qualification(core: Core, images: ImageAssets) -> (Self, ShellUpdates) {
        Self::from_core(core, images, None)
    }

    fn from_core(
        mut core: Core,
        images: ImageAssets,
        failure: Option<String>,
    ) -> (Self, ShellUpdates) {
        core.set_image_assets(images.clone());
        let mut entry = Entry::new(
            failure,
            crate::shell::selection::SelectionAllocator::default(),
        );
        let notifications = entry.sync(&core).into_iter().collect();
        entry.land(&mut core);
        let preferences = Preferences::from_core(&core);
        (
            Self {
                titlebar: crate::shell::titlebar::Titlebar::default(),
                core,
                entry,
                preferences,
                audio: editor_audio::Player::new(),
                audio_assets: crate::ui::media::audio::AudioAssets::new(),
                models: crate::ui::media::model::load::ModelAssets::new(),
                meshes_supported: std::rc::Rc::default(),
                meshes_changed: std::cell::Cell::new(false),
                dialogs: Default::default(),
                live: Default::default(),
                images,
            },
            ShellUpdates {
                notifications,
                preferences_changed: true,
                theme_transition: None,
                settings_platform_effects: Vec::new(),
                changed: true,
                live_only: false,
            },
        )
    }

    pub fn images(&self) -> &ImageAssets {
        &self.images
    }

    pub fn bind(&self, ctx: &AppCtx<'_>, window: AquaWindowId) {
        ctx.bind_mailbox(window, self.core.wake_mailbox());
        self.audio_assets.bind(ctx, window);
        self.models.bind(ctx, window);
        self.entry.bind(ctx, window);
        if let Some(palette) = self
            .entry
            .workspace()
            .and_then(|workspace| workspace.palette.as_ref())
        {
            if let Some(mailbox) = palette.wake_mailbox() {
                ctx.bind_mailbox(window, mailbox);
            }
        }
        if let Some(workspace) = self.entry.workspace() {
            workspace.inspector.bind(ctx, window);
            workspace.markdown.borrow().bind_media(ctx, window);
            ctx.bind_mailbox(window, workspace.images.wake_mailbox());
            for (_, _, content) in workspace.conversation.sources.iter() {
                ctx.bind_mailbox(window, content.action_wake());
                ctx.bind_mailbox(window, content.window().wake_mailbox());
            }
            workspace.bind_question_source(ctx, window);
            if let Some(source) = &workspace.conversation.question_pages.source_view {
                ctx.bind_mailbox(window, source.action_wake());
                ctx.bind_mailbox(window, source.window().wake_mailbox());
            }
            if let Some(mailbox) = workspace.conversation.question_pages.question_wake() {
                ctx.bind_mailbox(window, mailbox);
            }
            if let Some(browser) = workspace.conversation.question_pages.options_browser() {
                ctx.bind_mailbox(window, browser.borrow().window().wake_mailbox());
            }
            if let Some(mailbox) = workspace.usage.wake_mailbox() {
                ctx.bind_mailbox(window, mailbox);
            }
            ctx.bind_mailbox(window, workspace.marketplace.wake_mailbox());
            ctx.bind_mailbox(window, workspace.settings.wake_mailbox());
            ctx.bind_mailbox(window, workspace.home.wake_mailbox());
            if let Some(mailbox) = workspace.sidebar.wake_mailbox() {
                ctx.bind_mailbox(window, mailbox);
            }
            if let Some(mailbox) = workspace.settings_sidebar.wake_mailbox() {
                ctx.bind_mailbox(window, mailbox);
            }
            if let Some(owner) = &workspace.composer {
                if let Some(mailbox) = owner.answer_wake_mailbox() {
                    ctx.bind_mailbox(window, mailbox);
                }
                let composer = owner.controller.borrow();
                if let Some(mailbox) = composer.turn_wake_mailbox() {
                    ctx.bind_mailbox(window, mailbox);
                }
                if let Some(session) = &composer.session {
                    ctx.bind_mailbox(window, session.wake_mailbox());
                }
                if let Some(attachments) = &composer.attachments {
                    if let Some(mailbox) = attachments.wake_mailbox() {
                        ctx.bind_mailbox(window, mailbox);
                    }
                }
            }
        }
        if let Some(preview) = self
            .entry
            .workspace()
            .and_then(|workspace| workspace.resource_preview.as_ref())
        {
            if let Some(mailbox) = preview.wake_mailbox() {
                ctx.bind_mailbox(window, mailbox);
            }
        }
    }

    pub fn apply_preferences(
        &self,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> anyhow::Result<()> {
        self.preferences.apply_window(ctx, window)
    }

    pub fn drain(&mut self, window_active: bool, ui: &mut aqua::UiApp) -> ShellUpdates {
        let mut notifications = Vec::new();
        let tm_a = crate::shell::application::Tm::new("sd.assets");
        let mut changed =
            self.audio_assets.drain() | self.models.drain() | self.meshes_changed.replace(false);
        let mut preferences_changed = false;
        drop(tm_a);
        let tm_b = crate::shell::application::Tm::new("sd.entry");
        let (mut entry_effects, entry_changed) = self.entry.dispatch(&mut self.core);
        changed |= entry_changed;
        let (theme_transition, mut settings_platform_effects) =
            if let Some(workspace) = self.entry.workspace_mut() {
                changed |= workspace.dispatch_actions(&mut self.core);
                (
                    workspace.take_theme_transition(),
                    workspace.take_settings_platform_effects(),
                )
            } else {
                (None, Vec::new())
            };
        entry_effects.append(&mut settings_platform_effects);
        let settings_platform_effects = entry_effects;
        drop(tm_b);
        let tm_c = crate::shell::application::Tm::new("sd.events");
        let watched = entry_watched_thread(&self.entry);
        let entry = &mut self.entry;
        let live = &mut self.live;
        let now = std::time::Instant::now();
        const MESSAGES_PER_DRAIN: usize = 256;
        let mut live_changed = false;
        let mut exhausted = true;
        for _ in 0..MESSAGES_PER_DRAIN {
            let mut received = false;
            let pending = self.core.drain_step(|core, event| {
                received = true;
                match &event {
                    CoreEvent::Host {
                        host,
                        event: HostEvent::Live { thread_id, .. },
                    } if is_watched(watched.as_ref(), host, thread_id) => {
                        live_changed |= live.arrive(now)
                    }
                    CoreEvent::Host {
                        host,
                        event: HostEvent::Item(item),
                    } if is_streaming_prose(item) && is_watched(watched.as_ref(), host, &item.thread_id) => {
                        live_changed |= live.arrive(now)
                    }
                    _ => changed |= event_changes_view(&event, watched.as_ref()),
                };
                preferences_changed |= matches!(event, CoreEvent::SettingsChanged);
                if let Some(effects) = entry.handle_event(core, &event, window_active, ui) {
                    notifications.push(effects);
                }
                if let Some(workspace) = entry.workspace_mut() {
                    workspace.handle_domain_event(core, &event, window_active, ui);
                }
            });
            if !received && !pending {
                exhausted = false;
                break;
            }
        }
        if exhausted {
            self.core.wake();
        }
        live_changed |= self.live.due(now);
        drop(tm_c);
        let tm_d = crate::shell::application::Tm::new("sd.finish");
        self.entry.finish_events(&mut self.core, ui);
        changed |= self.entry.drain(&mut self.core);
        if preferences_changed {
            self.preferences = Preferences::from_core(&self.core);
        }
        drop(tm_d);
        let tm_e = crate::shell::application::Tm::new("sd.domains");
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.refresh_composer_mentions(&mut self.core, &self.preferences.theme());
            let domains_changed = workspace.drain_domains(&mut self.core, &self.images);
            changed |= domains_changed;
            changed |= workspace.markdown.borrow().drain_media();
            changed |= workspace.drain_palette(&self.core);
            changed |= workspace.drain_reference(&self.core);
        }
        drop(tm_e);
        ShellUpdates {
            notifications,
            preferences_changed,
            theme_transition,
            settings_platform_effects,
            changed: changed || live_changed,
            live_only: live_changed && !changed,
        }
    }

    pub fn open_palette(&mut self, ctx: &AppCtx<'_>, window: AquaWindowId) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.open_palette(&self.core);
        }
        self.bind(ctx, window);
    }

    pub fn open_reference(
        &mut self,
        ctx: &AppCtx<'_>,
        window: AquaWindowId,
        path: String,
        line: Option<u32>,
        scope: Option<(crate::core::HostId, artisan_api::ThreadId)>,
    ) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.open_reference(&self.core, path, line, scope);
        }
        self.bind(ctx, window);
    }

    pub fn dismiss_reference(&mut self) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.dismiss_reference();
        }
    }

    pub fn dismiss_palette(&mut self) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.dismiss_palette();
        }
    }

    pub fn activate_palette(&mut self) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.activate_palette(&mut self.core);
        }
    }

    pub fn new_thread(&mut self) {
        Workspace::new_thread(&mut self.core);
    }
    pub fn go_back(&mut self) {
        self.core.go_back();
    }
    pub fn open_settings(&mut self) {
        Workspace::open_settings(&mut self.core);
    }
    pub fn navigate(&mut self, screen: Screen) {
        self.core.navigate(screen);
    }
    pub fn quit(ctx: &mut AppCtx<'_>, _window: AquaWindowId) {
        ctx.quit();
    }
    pub fn close_window(ctx: &mut AppCtx<'_>, window: AquaWindowId) -> anyhow::Result<()> {
        ctx.request_window(WindowRequest::Close(window))?;
        Ok(())
    }
}

pub struct ShellResources {
    pub ui: aqua::UiApp,
    pub images: ImageAssets,
}

impl ShellResources {
    pub fn new(scale: f32) -> Option<Self> {
        Self::with_text(
            aqua_text::SharedFonts::new(),
            aqua_text::SharedRuns::new(),
            scale,
        )
    }

    pub fn for_runtime(runtime: &aqua::Aqua, scale: f32) -> Option<Self> {
        Self::with_text(runtime.fonts().clone(), runtime.runs().clone(), scale)
    }

    fn with_text(
        fonts: aqua_text::SharedFonts,
        runs: aqua_text::SharedRuns,
        scale: f32,
    ) -> Option<Self> {
        crate::ui::fonts::register_bundled_fonts(&fonts);
        let mut ui = aqua::UiApp::new(fonts, runs);
        ui.tree_mut().set_gpui_paint(true);
        if !ui.tree_mut().set_gpui_layout_scale(Some(scale)) {
            return None;
        }
        let images = ImageAssets::new(aqua::images::ImageAssetBudget::editor());
        Some(Self { ui, images })
    }

    pub fn with_color_profile(self, profile: aqua_core::ColorProfile) -> Self {
        Self {
            ui: aqua::UiApp::from_runtime(self.ui.into_runtime().with_color_profile(profile)),
            images: self.images,
        }
    }

    pub fn into_managed(self) -> aqua::images::ManagedUiApp {
        aqua::images::ManagedUiApp::new(self.ui, self.images)
    }
}

pub fn set_window_scale(ui: &mut aqua::UiApp, scale: f32) -> bool {
    ui.tree_mut().set_gpui_layout_scale(Some(scale))
}

fn entry_watched_thread(entry: &Entry) -> Option<(HostId, ThreadId)> {
    entry
        .workspace()
        .and_then(|workspace| workspace.conversation.open.clone())
}

pub fn event_changes_view(event: &CoreEvent, watched: Option<&(HostId, ThreadId)>) -> bool {
    match event {
        CoreEvent::Host {
            host,
            event: HostEvent::Live { thread_id, .. },
        } => is_watched(watched, host, thread_id),
        CoreEvent::Host {
            host,
            event: HostEvent::Item(item),
        } => is_watched(watched, host, &item.thread_id),
        _ => true,
    }
}

fn is_streaming_prose(item: &artisan_api::Item) -> bool {
    item.status == artisan_api::Status::Running
        && matches!(item.kind, artisan_api::ItemKind::AssistantMessage { .. })
}

fn is_watched(watched: Option<&(HostId, ThreadId)>, host: &HostId, thread: &ThreadId) -> bool {
    watched.is_some_and(|(open_host, open_thread)| open_host == host && open_thread == thread)
}
