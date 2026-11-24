use crate::core::{Core, CoreEvent, EditorFiles, HostEvent, HostId, Screen};
use crate::settings::about::BuildInfo;
use crate::shell::entry::Entry;
use crate::shell::refresh::{Cause, Invalidation};
use crate::shell::workspace::{NotificationEffects, Workspace};
use crate::ui::preferences::Preferences;
use aqua::AppCtx;
use aqua::images::ImageAssets;
use aqua::prelude::AquaWindowId;
use artisan_api::ThreadId;

pub(crate) const INTERFACE_LANGUAGE: &str = "en";
pub(crate) const WINDOW_NAME: &str = "Artisan Editor";

pub struct Shell {
    pub titlebar: crate::shell::titlebar::Titlebar,
    pub failures: crate::shell::action_failure::ActionFailureDialog,
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
    pub build: BuildInfo,
    pub images: ImageAssets,
}

impl std::fmt::Debug for Shell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct ShellUpdates {
    pub(crate) notifications: Vec<NotificationEffects>,
    pub(crate) preferences_changed: bool,
    pub(crate) theme_transition: Option<bool>,
    pub(crate) settings_platform_effects: Vec<crate::shell::workspace::SettingsPlatformEffect>,
    pub(crate) causes: Invalidation,
}

fn running_build() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        commit: env!("EDITOR_COMMIT").to_owned(),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .to_owned(),
        target: env!("EDITOR_TARGET").to_owned(),
        executable: match std::env::current_exe() {
            Ok(executable) => Some(executable),
            Err(error) => {
                tracing::warn!("find the running Editor's executable for About: {error}");
                None
            }
        },
        licences: crate::settings::licences::Licences::new(crate::setup::licences),
    }
}

impl Shell {
    pub fn new(files: EditorFiles, images: ImageAssets) -> (Self, ShellUpdates) {
        Self::opening(files, images, None)
    }

    pub(crate) fn opening(
        files: EditorFiles,
        images: ImageAssets,
        link: Option<Screen>,
    ) -> (Self, ShellUpdates) {
        aqua::native::startup_mark("core_init_begin");
        let (mut core, failures) = Core::init(files);
        aqua::native::startup_mark("core_init_done");
        if let Some(screen) = link {
            core.navigate(screen);
        }
        let failure = (!failures.is_empty()).then(|| {
            let causes: Vec<String> = failures
                .iter()
                .map(|failure| ::diagnostics::chain(failure))
                .collect();
            crate::core::Problem::detailed(
                "Couldn't read this device's Artisan files. They are kept as they are until they can be read.",
                causes.join("; "),
            )
        });
        Self::from_core(core, images, failure, running_build())
    }

    #[cfg(feature = "qualification")]
    pub(crate) fn for_qualification(core: Core, images: ImageAssets) -> (Self, ShellUpdates) {
        Self::from_core(core, images, None, crate::qualification::build())
    }

    fn from_core(
        mut core: Core,
        images: ImageAssets,
        failure: Option<crate::core::Problem>,
        build: BuildInfo,
    ) -> (Self, ShellUpdates) {
        core.set_image_assets(images.clone());
        let mut entry = Entry::new(
            failure,
            crate::shell::selection::SelectionAllocator::default(),
        );
        let notifications = entry.sync(&core).into_iter().collect();
        entry.land(&mut core);
        let preferences =
            Preferences::from_core(&core, editor_ui::preferences::SystemAccess::default());
        (
            Self {
                titlebar: crate::shell::titlebar::Titlebar::default(),
                failures: crate::shell::action_failure::ActionFailureDialog::default(),
                core,
                entry,
                preferences,
                audio: editor_audio::Player::new(),
                audio_assets: crate::ui::media::audio::AudioAssets::new(),
                models: crate::ui::media::model::load::ModelAssets::new(),
                meshes_supported: std::rc::Rc::default(),
                meshes_changed: std::cell::Cell::new(false),
                dialogs: crate::ui::dialogs::DialogRoutes::default(),
                live: crate::shell::live_throttle::LiveThrottle::default(),
                build,
                images,
            },
            ShellUpdates {
                notifications,
                preferences_changed: true,
                theme_transition: None,
                settings_platform_effects: Vec::new(),
                causes: Invalidation::default(),
            },
        )
    }

    pub fn images(&self) -> &ImageAssets {
        &self.images
    }

    pub(crate) fn bind(&self, ctx: &AppCtx<'_>, window: AquaWindowId) {
        ctx.bind_mailbox(window, self.core.wake_mailbox());
        self.audio_assets.bind(ctx, window);
        self.models.bind(ctx, window);
        self.entry.bind(ctx, window);
        if let Some(palette) = self
            .entry
            .workspace()
            .and_then(|workspace| workspace.palette.as_ref())
            && let Some(mailbox) = palette.wake_mailbox()
        {
            ctx.bind_mailbox(window, mailbox);
        }
        if let Some(workspace) = self.entry.workspace() {
            ctx.bind_mailbox(window, workspace.draft_wake_mailbox());
            workspace.inspector.bind(ctx, window);
            if let Some(mailbox) = workspace.conversation.wake_mailbox() {
                ctx.bind_mailbox(window, mailbox);
            }
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
                if let Some(attachments) = &composer.attachments
                    && let Some(mailbox) = attachments.wake_mailbox()
                {
                    ctx.bind_mailbox(window, mailbox);
                }
            }
        }
        if let Some(preview) = self
            .entry
            .workspace()
            .and_then(|workspace| workspace.resource_preview.as_ref())
            && let Some(mailbox) = preview.wake_mailbox()
        {
            ctx.bind_mailbox(window, mailbox);
        }
    }

    pub(crate) fn apply_preferences(
        &self,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> Result<(), editor_ui::preferences::ApplyWindowError> {
        self.preferences.apply_window(ctx, window)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one bounded drain over core, entry and live events sharing the frame budget"
    )]
    pub(crate) fn drain(&mut self, window_active: bool, ui: &mut aqua::UiApp) -> ShellUpdates {
        const MESSAGES_PER_DRAIN: usize = 256;
        const DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);
        let mut notifications = Vec::new();
        let mut causes = Invalidation::default();
        causes.note(
            Cause::MediaAssets,
            self.audio_assets.drain() | self.models.drain() | self.meshes_changed.replace(false),
        );
        let mut preferences_changed = false;
        let (mut entry_effects, entry_changed) = self.entry.dispatch(&mut self.core);
        causes.note(Cause::EntryActions, entry_changed);
        let (theme_transition, mut settings_platform_effects) =
            if let Some(workspace) = self.entry.workspace_mut() {
                causes.note(
                    Cause::WorkspaceActions,
                    workspace.dispatch_actions(&mut self.core),
                );
                (
                    workspace.take_theme_transition(),
                    workspace.take_settings_platform_effects(),
                )
            } else {
                (None, Vec::new())
            };
        entry_effects.append(&mut settings_platform_effects);
        let mut settings_platform_effects = entry_effects;
        let watched = entry_watched_thread(&self.entry);
        let thread_on_screen = matches!(self.core.screen(), Screen::Thread { .. });
        let entry = &mut self.entry;
        let live = &mut self.live;
        let clock = self.core.clock().clone();
        let now = clock.now();
        let mut exhausted = true;
        let mut live_events = super::live_batch::LiveBatch::default();
        let mut apply_event = |core: &mut Core, event: CoreEvent| {
            match event_cause(&event, watched.as_ref(), thread_on_screen) {
                Some(Cause::LiveText) => causes.note(Cause::LiveText, live.arrive(now)),
                Some(cause) => causes.note(cause, true),
                None => false,
            };
            preferences_changed |= matches!(event, CoreEvent::SettingsChanged);
            if let CoreEvent::OpenUrl(url) = &event {
                settings_platform_effects.push(
                    crate::shell::workspace::SettingsPlatformEffect::OpenUrl(url.clone()),
                );
            }
            if let Some(effects) = entry.handle_event(core, &event, window_active, ui) {
                notifications.push(effects);
            }
            if let Some(workspace) = entry.workspace_mut() {
                workspace.handle_domain_event(core, &event, window_active, ui);
            }
        };
        for _ in 0..MESSAGES_PER_DRAIN {
            let mut received = false;
            let pending = self.core.drain_step(|core, event| {
                received = true;
                if let Some(event) = live_events.push(event) {
                    for pending in live_events.take() {
                        apply_event(core, pending);
                    }
                    apply_event(core, event);
                }
            });
            if !received && !pending {
                exhausted = false;
                break;
            }
            if pending && clock.now().saturating_duration_since(now) >= DRAIN_BUDGET {
                break;
            }
        }
        for event in live_events.take() {
            apply_event(&mut self.core, event);
        }
        if exhausted {
            self.core.wake();
        }
        causes.note(Cause::LiveText, self.live.due(now));
        self.entry.finish_events(&mut self.core, ui);
        causes.note(Cause::EntryState, self.entry.drain(&mut self.core));
        if preferences_changed {
            self.preferences = Preferences::from_core(&self.core, self.preferences.system);
        }
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.refresh_composer_mentions(&mut self.core, &self.preferences.theme());
            causes.extend(workspace.drain_domains(&mut self.core, &self.images));
            causes.note(
                Cause::MarkdownMedia,
                workspace.markdown.borrow().drain_media(),
            );
            causes.note(Cause::PaletteState, workspace.drain_palette(&self.core));
            causes.note(Cause::ReferenceState, workspace.drain_reference(&self.core));
        }
        ShellUpdates {
            notifications,
            preferences_changed,
            theme_transition,
            settings_platform_effects,
            causes,
        }
    }

    pub(crate) fn open_palette(&mut self, ctx: &AppCtx<'_>, window: AquaWindowId) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.open_palette(&self.core);
        }
        self.bind(ctx, window);
    }

    pub(crate) fn open_reference(
        &mut self,
        ctx: &AppCtx<'_>,
        window: AquaWindowId,
        path: String,
        line: Option<u32>,
        scope: Option<(HostId, ThreadId)>,
    ) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.open_reference(&self.core, path, line, scope);
        }
        self.bind(ctx, window);
    }

    pub(crate) fn retry_reference(&mut self) -> bool {
        let core = &self.core;
        self.entry
            .workspace_mut()
            .and_then(|workspace| workspace.resource_preview.as_mut())
            .is_some_and(|preview| preview.retry(core))
    }

    pub(crate) fn dismiss_reference(&mut self) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.dismiss_reference();
        }
    }

    pub(crate) fn dismiss_palette(&mut self) {
        if let Some(workspace) = self.entry.workspace_mut() {
            workspace.dismiss_palette();
        }
    }

    pub(crate) fn new_thread(&mut self) {
        Workspace::new_thread(&mut self.core);
    }
    pub(crate) fn go_back(&mut self) {
        self.core.go_back();
    }
    pub(crate) fn open_settings(&mut self) {
        Workspace::open_settings(&mut self.core);
    }
    pub fn navigate(&mut self, screen: Screen) {
        self.core.navigate(screen);
    }
}

pub struct ShellResources {
    pub ui: aqua::UiApp,
    pub images: ImageAssets,
}

impl std::fmt::Debug for ShellResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellResources").finish_non_exhaustive()
    }
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

    pub(crate) fn with_color_profile(self, profile: aqua_core::ColorProfile) -> Self {
        Self {
            ui: aqua::UiApp::from_runtime(self.ui.into_runtime().with_color_profile(profile)),
            images: self.images,
        }
    }

    pub(crate) fn into_managed(self) -> aqua::images::ManagedUiApp {
        aqua::images::ManagedUiApp::new(self.ui, self.images)
    }
}

pub(crate) fn set_window_scale(ui: &mut aqua::UiApp, scale: f32) -> bool {
    ui.tree_mut().set_gpui_layout_scale(Some(scale))
}

fn entry_watched_thread(entry: &Entry) -> Option<(HostId, ThreadId)> {
    entry
        .workspace()
        .and_then(|workspace| workspace.conversation.open.clone())
}

pub fn event_cause(
    event: &CoreEvent,
    watched: Option<&(HostId, ThreadId)>,
    thread_on_screen: bool,
) -> Option<Cause> {
    match event {
        CoreEvent::Host {
            host,
            event: HostEvent::Live { thread_id, .. },
        } => is_watched(watched, host, thread_id).then_some(Cause::LiveText),
        CoreEvent::Host {
            host,
            event: HostEvent::Item(item),
        } => match (
            is_watched(watched, host, &item.thread_id),
            is_streaming_prose(item),
        ) {
            (false, _) => None,
            (true, true) => Some(Cause::LiveText),
            (true, false) => Some(Cause::ThreadItems),
        },
        CoreEvent::Host {
            host,
            event:
                HostEvent::ThreadChanged { thread_id } | HostEvent::ThreadSummaryChanged { thread_id },
        } => match (thread_on_screen, is_watched(watched, host, thread_id)) {
            (true, false) => Some(Cause::ListedThreads),
            (false, _) | (true, true) => Some(Cause::OpenThread),
        },
        CoreEvent::ScreenChanged
        | CoreEvent::HostsChanged
        | CoreEvent::SettingsChanged
        | CoreEvent::AccessChanged
        | CoreEvent::McpSignInChanged
        | CoreEvent::OpenUrl(_)
        | CoreEvent::Account(_)
        | CoreEvent::Host { .. } => Some(Cause::HostState),
    }
}

fn is_streaming_prose(item: &artisan_api::Item) -> bool {
    item.status == artisan_api::Status::Running
        && matches!(item.kind, artisan_api::ItemKind::AssistantMessage { .. })
}

fn is_watched(watched: Option<&(HostId, ThreadId)>, host: &HostId, thread: &ThreadId) -> bool {
    watched.is_some_and(|(open_host, open_thread)| open_host == host && open_thread == thread)
}
