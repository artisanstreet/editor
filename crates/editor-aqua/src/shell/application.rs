use crate::shell::host::{Shell, ShellResources, ShellUpdates, set_window_scale};
use crate::shell::native_notifications::NativeNotifications;
use crate::shell::refresh::{Cause, Invalidation, Refresh};
use aqua::prelude::{
    A11yActionRequest, A11yNode, AquaWindowId, CursorIcon, DragRegions, FrameReason, InputEvent,
    Instant, ManagedUiApp, NodeId, ObservedWindowState, Scene, WindowEvent,
};
use aqua::{AppCtx, Application, ExternalFrame, UiApp};
use aqua_ui::element::Element;

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("the account workspace is gone")]
    NoWorkspace,
    #[error("Forge {host} is gone")]
    HostGone { host: crate::core::HostId },
    #[error("the conversation's Forge lifetime is gone")]
    ConversationGone,
    #[error("thread {thread} on Forge {host} has no action scope")]
    NoActionScope {
        host: crate::core::HostId,
        thread: artisan_api::ThreadId,
    },
    #[error("configure the conversation rows")]
    Rows(#[source] crate::conversation::view::factory::ConfigureRowsError),
    #[error("the conversation composer is not mounted")]
    NoComposer,
    #[error("the composer has no Forge lifetime on {host}")]
    ComposerHostGone { host: crate::core::HostId },
    #[error("register the {surface} for selection")]
    Selection {
        surface: &'static str,
        #[source]
        source: aqua_text::document::SelectionSourceError,
    },
    #[error("show the inspector")]
    Inspector(#[source] crate::inspector::BuildInspectorError),
    #[error("show the resource preview")]
    ResourcePreview(#[source] crate::shell::resource_preview::view::UnpreparedError),
}

pub trait SectionViews {
    fn take_invalidation(&mut self) -> Invalidation {
        Invalidation::default()
    }
    fn advance_glides(&mut self, _shell: &mut Shell, _ui: &mut UiApp, _now: Instant) -> bool {
        false
    }
    fn glide_active(&self) -> bool {
        false
    }
    fn take_events(
        &mut self,
        _shell: &mut Shell,
        _ui: &UiApp,
        _ctx: &mut AppCtx<'_>,
        _window: AquaWindowId,
    ) -> bool {
        false
    }
    fn animate(
        &mut self,
        _shell: &mut Shell,
        _ui: &mut UiApp,
        _ctx: &mut AppCtx<'_>,
        _window: AquaWindowId,
    ) -> bool {
        false
    }
    fn pre_input(
        &mut self,
        _shell: &Shell,
        _ui: &mut UiApp,
        _input: &InputEvent,
        _now: Instant,
    ) -> Option<bool> {
        None
    }
    fn open_composer_images(&mut self, _shell: &mut Shell, _ui: &mut UiApp) -> bool {
        false
    }
    fn reader_moved_transcript(&mut self) {}
    fn resize_needs_rebuild(&self, _shell: &Shell, _window: &ObservedWindowState) -> bool {
        true
    }
    fn refresh_transcript(
        &mut self,
        _shell: &mut Shell,
        _ui: &mut UiApp,
        _window: &ObservedWindowState,
    ) -> bool {
        false
    }
    fn refresh_sidebar_frame(
        &self,
        _shell: &Shell,
        _ui: &mut UiApp,
        _window: &ObservedWindowState,
    ) -> bool {
        false
    }
    fn refresh_sidebar(
        &self,
        _shell: &Shell,
        _ui: &mut UiApp,
        _window: &ObservedWindowState,
    ) -> bool {
        false
    }
    fn build(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> Result<Element, BuildError>;
    fn input(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        input: &InputEvent,
        consumed: bool,
    ) -> bool;
    fn wake(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool;
    fn mounted(
        &mut self,
        _shell: &mut Shell,
        _ui: &mut UiApp,
        _ctx: &mut AppCtx<'_>,
        _window: AquaWindowId,
    ) {
    }
    fn frame_completed(
        &mut self,
        _shell: &mut Shell,
        _ui: &mut UiApp,
        _ctx: &mut AppCtx<'_>,
        _window: AquaWindowId,
        _sequence: u64,
    ) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotificationStart {
    Disabled,
    Pending,
    Started,
}

pub struct ShellApplication<V> {
    pub shell: Shell,
    pub views: V,
    pub ui: ManagedUiApp,
    pub notifications: Option<NativeNotifications>,
    pub notifications_start: NotificationStart,
    pub initial: Option<ShellUpdates>,
    pub mounted: bool,
    pub font_popup: Option<crate::ui::popover::AnimatedPickerHost>,
    pub streaming_popup: Option<crate::ui::popover::AnimatedPickerHost>,
    pub text_size_popup: Option<crate::ui::popover::AnimatedPickerHost>,
    pub contrast_popup: Option<crate::ui::popover::AnimatedPickerHost>,
    pub retention_popup: Option<crate::ui::popover::AnimatedPickerHost>,
    pub usage_deadline: Option<Instant>,
    pub fit_launch: bool,
    #[cfg(feature = "capture")]
    pub render_tap: Option<std::rc::Rc<std::cell::RefCell<Vec<aqua_protocol::RenderCommand>>>>,
}

impl<V> std::fmt::Debug for ShellApplication<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShellApplication").finish_non_exhaustive()
    }
}

#[cfg(feature = "qualification")]
impl ShellApplication<crate::shell::root::RootViews> {
    pub fn qualification_observation(
        &self,
    ) -> Option<crate::qualification::observation::Observation> {
        self.views
            .qualification_observation(&self.shell, self.ui.ui())
    }
}

impl<V: SectionViews> ShellApplication<V> {
    fn apply_motion_policy(&mut self, ctx: &AppCtx<'_>) {
        let policy = match self.shell.preferences.motion() {
            editor_ui::theme::Motion::Reduced => aqua_ui::motion::ReducedMotion::Reduce,
            editor_ui::theme::Motion::Full => aqua_ui::motion::ReducedMotion::Full,
        };
        self.ui
            .ui_mut()
            .host_mut()
            .motion_mut()
            .set_reduced_motion(policy, ctx.now().as_secs_f64());
    }

    fn refresh_sections(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        refresh: Refresh,
    ) -> bool {
        if !self.mounted {
            return false;
        }
        let Some(window) = ctx.window_state(id).cloned() else {
            return false;
        };
        let sidebar = matches!(refresh, Refresh::Sidebar | Refresh::Sections);
        let sidebar_frame = matches!(refresh, Refresh::SidebarFrame | Refresh::FrameSections);
        let transcript = matches!(
            refresh,
            Refresh::Transcript | Refresh::Sections | Refresh::FrameSections
        );
        let sidebar_done = if sidebar_frame {
            self.views
                .refresh_sidebar_frame(&self.shell, self.ui.ui_mut(), &window)
        } else {
            !sidebar
                || self
                    .views
                    .refresh_sidebar(&self.shell, self.ui.ui_mut(), &window)
        };
        let transcript_done = !transcript
            || self
                .views
                .refresh_transcript(&mut self.shell, self.ui.ui_mut(), &window);
        let updated = (sidebar || sidebar_frame || transcript) && sidebar_done && transcript_done;
        if updated {
            crate::core::diagnostics::rebuild_stats::record_event("section_refresh");
            ctx.request_frame(id, FrameReason::ANIMATION);
        }
        updated
    }

    fn advance_hover_animations(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) {
        for popup in [
            &mut self.font_popup,
            &mut self.streaming_popup,
            &mut self.text_size_popup,
            &mut self.contrast_popup,
            &mut self.retention_popup,
        ]
        .into_iter()
        .flatten()
        {
            if popup.is_open(self.ui.ui()) {
                if let Err(error) = popup.advance(self.ui.ui_mut(), ctx.now().as_secs_f64()) {
                    tracing::warn!(%error, "could not animate settings popup");
                }
                if popup.is_animating(self.ui.ui()) {
                    ctx.request_frame(id, FrameReason::ANIMATION);
                }
            }
        }
        if let Some(owner) = self
            .shell
            .entry
            .workspace_mut()
            .and_then(|workspace| workspace.composer.as_mut())
        {
            if let Err(error) = owner.advance_popups(self.ui.ui_mut(), ctx.now().as_secs_f64()) {
                tracing::warn!(%error, "could not advance composer popup");
            }
            owner.advance_note_ticker(self.ui.ui_mut(), ctx.now().as_secs_f64());
            owner.advance_primary(self.ui.ui_mut(), self.shell.core.clock().now());
            if owner.needs_animation(self.ui.ui()) {
                ctx.request_frame(id, FrameReason::ANIMATION);
            }
        }
        let reduced =
            self.ui.ui().host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let frame = editor_ui::kit::hover::live::advance(
            self.ui.ui_mut(),
            self.shell.core.clock().now(),
            reduced,
        );
        if frame.changed || frame.animating {
            ctx.request_frame(id, FrameReason::ANIMATION);
        }
    }

    pub fn new(files: crate::core::EditorFiles, resources: ShellResources, views: V) -> Self {
        let (shell, initial) = Shell::new(files, resources.images.clone());
        Self::from_shell(shell, initial, resources, views)
    }

    pub(crate) fn opening(
        files: crate::core::EditorFiles,
        resources: ShellResources,
        views: V,
        link: Option<crate::core::Screen>,
    ) -> Self {
        let (shell, initial) = Shell::opening(files, resources.images.clone(), link);
        Self::from_shell(shell, initial, resources, views)
    }

    #[cfg(feature = "qualification")]
    pub fn for_qualification(core: crate::core::Core, resources: ShellResources, views: V) -> Self {
        let (shell, initial) = Shell::for_qualification(core, resources.images.clone());
        let mut app = Self::from_shell(shell, initial, resources, views);
        app.notifications_start = NotificationStart::Disabled;
        app
    }

    fn from_shell(
        shell: Shell,
        initial: ShellUpdates,
        resources: ShellResources,
        views: V,
    ) -> Self {
        Self {
            shell,
            views,
            ui: resources.into_managed(),
            notifications: None,
            notifications_start: NotificationStart::Pending,
            initial: Some(initial),
            mounted: false,
            font_popup: None,
            streaming_popup: None,
            text_size_popup: None,
            contrast_popup: None,
            retention_popup: None,
            usage_deadline: None,
            fit_launch: false,
            #[cfg(feature = "capture")]
            render_tap: None,
        }
    }

    #[cfg(all(windows, feature = "capture"))]
    pub(crate) fn tap_render(
        &mut self,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<aqua_protocol::RenderCommand>>> {
        std::rc::Rc::clone(self.render_tap.get_or_insert_with(Default::default))
    }

    pub(crate) fn fitting_launch_to_display(mut self) -> Self {
        self.fit_launch = true;
        self
    }

    fn start_notifications(&mut self, ctx: &AppCtx<'_>, id: AquaWindowId) -> bool {
        if self.notifications_start != NotificationStart::Pending {
            return false;
        }
        self.notifications_start = NotificationStart::Started;
        match NativeNotifications::new() {
            Ok(notifications) => {
                notifications.bind(ctx, id);
                let pending = notifications.has_pending();
                self.notifications = Some(notifications);
                pending
            }
            Err(error) => {
                tracing::warn!(%error, "system notifications unavailable");
                false
            }
        }
    }

    fn update(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, updates: ShellUpdates) -> bool {
        let mut failed = false;
        for effect in updates.settings_platform_effects {
            match effect {
                crate::shell::workspace::SettingsPlatformEffect::OpenUrl(url) => {
                    if let Err(error) = crate::shell::open_url(&url) {
                        self.shell.failures.link(url, &error);
                        failed = true;
                    }
                }
                crate::shell::workspace::SettingsPlatformEffect::OpenNotices => {
                    if let Err(error) = crate::setup::open_notices() {
                        self.shell.failures.notices(&error);
                        failed = true;
                    }
                }
                crate::shell::workspace::SettingsPlatformEffect::CopyText(text) => {
                    if let Err(error) = ctx.clipboard().set_text(text) {
                        self.shell
                            .failures
                            .copy(crate::shell::action_failure::CopySubject::Text, &error);
                        failed = true;
                    }
                }
            }
        }
        if let Some(dark) = updates.theme_transition {
            let reduced = self.ui.ui().host().motion().reduced_motion()
                == aqua_ui::motion::ReducedMotion::Reduce;
            begin_theme_transition(ctx, id, dark, reduced);
        }
        if updates.preferences_changed {
            self.apply_motion_policy(ctx);
            if let Err(error) = self.shell.apply_preferences(ctx, id) {
                tracing::error!(
                    error = %::diagnostics::chain(&error),
                    "could not apply window preferences"
                );
            }
        }
        if !updates.notifications.is_empty() {
            self.start_notifications(ctx, id);
        }
        if let Some(center) = &mut self.notifications {
            for effects in updates.notifications {
                if let Err(error) = center.apply(effects, &self.shell, ctx, id) {
                    tracing::warn!(
                        error = %::diagnostics::chain(&error),
                        "could not apply notification effects"
                    );
                }
            }
        }
        failed
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one rebuild pass: popups, views, mount and the frame request in order"
    )]
    fn rebuild(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        reason: FrameReason,
        invalidation: &Invalidation,
    ) {
        let _tm = Tm::new("rebuild");
        let _rebuild_span = crate::core::diagnostics::rebuild_stats::span("rb_total");
        self.shell.live.flushed(self.shell.core.clock().now());
        let mut reasons = invalidation.labels();
        reasons.extend(crate::core::diagnostics::rebuild_stats::take_notes());
        crate::core::diagnostics::rebuild_stats::record_rebuild(&reasons);
        let Some(window) = ctx.window_state(id).cloned() else {
            return;
        };
        let was_mounted = self.mounted;
        self.ui
            .ui_mut()
            .set_selection_background(Some(crate::ui::chrome::card::polar_selection()));
        self.ui.ui_mut().set_selection_ink(Some(
            crate::ui::chrome::card::POLAR_INK.to_reference_paint(),
        ));
        let epoch = self.ui.ui().tree().invalidation_epoch();
        let reduced =
            self.ui.ui().host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        if let Some(workspace) = self.shell.entry.workspace_mut() {
            workspace.marketplace.reduced_motion = reduced;
            if let Some(owner) = &mut workspace.composer {
                owner.set_reduced_motion(reduced);
            }
        }
        if let Some(workspace) = self.shell.entry.workspace_mut() {
            if workspace.conversation.question_pages.source_view.is_none() {
                workspace.release_question_source(self.ui.ui_mut());
            }
            if workspace
                .conversation
                .question_pages
                .options_browser()
                .is_none()
            {
                workspace.release_question_options(self.ui.ui_mut());
            }
        }
        aqua_ui::style::set_root_letter_spacing_em(Some(
            self.shell.preferences.theme().typography.tracking_em,
        ));
        let build_span = crate::core::diagnostics::rebuild_stats::span("rb_build");
        let built = self.views.build(&mut self.shell, self.ui.ui_mut(), &window);
        drop(build_span);
        aqua_core::checkpoint::reach();
        let element = match built {
            Ok(element) => window_root(element),
            Err(error) => {
                let problem = crate::core::Problem::caused_by(
                    "Something went wrong drawing this view.",
                    &error,
                );
                tracing::error!(error = ?problem.detail(), "could not build Editor view");
                self.show_failure(&problem, ctx, id, reason);
                self.shell.bind(ctx, id);
                return;
            }
        };
        let refresh_span = crate::core::diagnostics::rebuild_stats::span("rb_refresh");
        let result = if self.mounted {
            self.ui.ui_mut().refresh(vec![element])
        } else {
            self.ui.ui_mut().mount(|_| element)
        };
        drop(refresh_span);
        aqua_core::checkpoint::reach();
        match result {
            Ok(()) => {
                self.mounted = true;
                self.shell.entry.sync_editors(self.ui.ui_mut());
                let mounted_span = crate::core::diagnostics::rebuild_stats::span("rb_mounted");
                self.views
                    .mounted(&mut self.shell, self.ui.ui_mut(), ctx, id);
                drop(mounted_span);
                aqua_core::checkpoint::reach();
                let settings_visible =
                    matches!(self.shell.core.screen(), crate::core::Screen::Settings(_));
                let assets = self.shell.images().clone();
                if let Some(workspace) = self.shell.entry.workspace_mut() {
                    workspace.sync_marketplace_ui(self.ui.ui_mut());
                    workspace.sync_editor_ui(self.ui.ui_mut(), settings_visible);
                    workspace
                        .sync_terminal_dialog(self.ui.ui_mut(), &self.shell.preferences.theme());
                    workspace.inspector.reveal_tail(self.ui.ui_mut());
                    if workspace.sync_composer_popups(
                        &self.shell.core,
                        self.ui.ui_mut(),
                        &self.shell.preferences.theme(),
                        &assets,
                        ctx.now().as_secs_f64(),
                    ) {
                        ctx.request_frame(id, FrameReason::ANIMATION);
                    }
                }
                aqua_core::checkpoint::reach();
                self.sync_settings_popups(ctx.now().as_secs_f64());
                if matches!(
                    self.shell.core.workspace_screen(),
                    crate::core::Screen::Usage
                ) {
                    let deadline = *self.usage_deadline.get_or_insert_with(|| {
                        ctx.now() + crate::sidebar::usage::controller::CLOCK_INTERVAL
                    });
                    ctx.request_animation_deadline(id, deadline);
                } else {
                    self.usage_deadline = None;
                }
                if let Some(at) = self
                    .shell
                    .entry
                    .workspace()
                    .and_then(|workspace| workspace.sidebar.next_tick())
                {
                    let delay = std::time::Duration::from_millis(
                        u64::try_from(at.saturating_sub(self.shell.core.clock().timestamp()))
                            .unwrap_or(0),
                    );
                    ctx.request_animation_deadline(id, ctx.now() + delay);
                }
                if let Some(delay) = self
                    .shell
                    .entry
                    .workspace()
                    .and_then(|workspace| workspace.conversation.landing_wait())
                {
                    ctx.request_animation_deadline(id, ctx.now() + delay);
                }
                if !was_mounted || self.ui.ui().tree().invalidation_epoch() != epoch {
                    ctx.request_frame(id, reason);
                }
            }
            Err(error) => {
                tracing::error!(%error, "could not mount Editor view");
                self.show_failure(
                    &crate::core::Problem::caused_by(
                        "Something went wrong drawing this view.",
                        &error,
                    ),
                    ctx,
                    id,
                    reason,
                );
            }
        }
        self.shell.bind(ctx, id);
    }

    fn refresh_transcript(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        reason: FrameReason,
    ) -> bool {
        let Some(window) = ctx.window_state(id).cloned() else {
            return false;
        };
        let _tm = Tm::new("rebuild_transcript");
        let epoch = self.ui.ui().tree().invalidation_epoch();
        let refreshed = self
            .views
            .refresh_transcript(&mut self.shell, self.ui.ui_mut(), &window);
        if !refreshed {
            return false;
        }
        crate::core::diagnostics::rebuild_stats::record_event("transcript_subtree");
        self.shell.live.flushed(self.shell.core.clock().now());
        self.views
            .mounted(&mut self.shell, self.ui.ui_mut(), ctx, id);
        if self.ui.ui().tree().invalidation_epoch() != epoch {
            ctx.request_frame(id, reason);
        }
        self.shell.bind(ctx, id);
        true
    }

    fn refresh_drained(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        refresh: Refresh,
    ) -> bool {
        match refresh {
            Refresh::None | Refresh::Workspace => false,
            Refresh::Transcript => self.refresh_transcript(ctx, id, FrameReason::INPUT),
            Refresh::Sidebar | Refresh::SidebarFrame => self.refresh_sections(ctx, id, refresh),
            Refresh::Sections | Refresh::FrameSections => {
                let refreshed = self.refresh_sections(ctx, id, refresh);
                if refreshed {
                    self.shell.live.flushed(self.shell.core.clock().now());
                    self.views
                        .mounted(&mut self.shell, self.ui.ui_mut(), ctx, id);
                    self.shell.bind(ctx, id);
                }
                refreshed
            }
        }
    }

    fn show_failure(
        &mut self,
        problem: &crate::core::Problem,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        reason: FrameReason,
    ) {
        let card = failure_card(problem, &self.shell.preferences.theme());
        let shown = if self.mounted {
            self.ui.ui_mut().refresh(vec![card])
        } else {
            self.ui.ui_mut().mount(|_| card)
        };
        match shown {
            Ok(()) => self.mounted = true,
            Err(error) => tracing::error!(%error, "could not show failure card"),
        }
        ctx.request_frame(id, reason);
    }

    fn sync_settings_popups(&mut self, now: f64) {
        let Some(workspace) = self.shell.entry.workspace() else {
            close_popup(self.ui.ui_mut(), &mut self.font_popup);
            close_popup(self.ui.ui_mut(), &mut self.streaming_popup);
            close_popup(self.ui.ui_mut(), &mut self.text_size_popup);
            close_popup(self.ui.ui_mut(), &mut self.contrast_popup);
            close_popup(self.ui.ui_mut(), &mut self.retention_popup);
            return;
        };
        let settings_open = matches!(self.shell.core.screen(), crate::core::Screen::Settings(_));
        let font_open = settings_open && workspace.settings.font_select.borrow().is_open();
        let streaming_open =
            settings_open && workspace.settings.streaming_select.borrow().is_open();
        let text_size_open =
            settings_open && workspace.settings.text_size_select.borrow().is_open();
        let contrast_open = settings_open && workspace.settings.contrast_select.borrow().is_open();
        let retention_open =
            settings_open && workspace.settings.retention_select.borrow().is_open();
        let theme = self.shell.preferences.theme();
        let font = font_open.then(|| {
            crate::settings::view::font_menu(
                &workspace.settings,
                &self.shell.core,
                &theme,
                workspace.settings_emit(),
            )
            .into_parts()
        });
        let streaming = streaming_open.then(|| {
            crate::settings::view::streaming_menu(
                &workspace.settings,
                &self.shell.core,
                &theme,
                workspace.settings_emit(),
            )
            .into_parts()
        });
        let text_size = text_size_open.then(|| {
            crate::settings::view::text_size_menu(
                &workspace.settings,
                &self.shell.core,
                &theme,
                workspace.settings_emit(),
            )
            .into_parts()
        });
        let contrast = contrast_open.then(|| {
            crate::settings::view::high_contrast_menu(
                &workspace.settings,
                &self.shell.core,
                &theme,
                workspace.settings_emit(),
            )
            .into_parts()
        });
        let retention = if retention_open {
            self.shell.core.active_host().map(|host| {
                crate::settings::archive_view::retention_menu(
                    &workspace.settings,
                    host,
                    &theme,
                    workspace.settings_emit(),
                )
                .into_parts()
            })
        } else {
            None
        };
        let ui = self.ui.ui_mut();
        for (popup, key, content) in [
            (&mut self.font_popup, "settings-font-set", font),
            (
                &mut self.streaming_popup,
                "settings-streaming-mode",
                streaming,
            ),
            (&mut self.text_size_popup, "settings-text-size", text_size),
            (&mut self.contrast_popup, "settings-high-contrast", contrast),
            (
                &mut self.retention_popup,
                "settings-thread-retention",
                retention,
            ),
        ] {
            sync_popup(ui, popup, key, content, now);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one drain pass over shell, views and native effects with shared timing"
    )]
    fn drain(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, sections_changed: bool) {
        let tm_pre = Tm::new("drain.pre");
        for control in self.shell.titlebar.take_actions() {
            let request = match control {
                aqua::prelude::CaptionControl::Minimize => {
                    aqua::prelude::WindowRequest::SetMinimized(id, true)
                }
                aqua::prelude::CaptionControl::Maximize => {
                    let Some(window) = ctx.window_state(id) else {
                        continue;
                    };
                    aqua::prelude::WindowRequest::SetMaximized(id, !window.maximized)
                }
                aqua::prelude::CaptionControl::Close => aqua::prelude::WindowRequest::Close(id),
            };
            if let Err(error) = ctx.request_window(request) {
                tracing::warn!(%error, "could not apply caption action");
            }
        }
        let active = ctx.window_state(id).is_some_and(|window| window.focused);
        if let Some(notifications) = &mut self.notifications
            && let Err(error) = notifications.drain(&mut self.shell, ctx, id)
        {
            tracing::warn!(
                error = %::diagnostics::chain(&error),
                "could not activate notification"
            );
        }
        drop(tm_pre);
        let mut causes = Invalidation::default();
        let tm_workspace = Tm::new("drain.workspace");
        let noted = self.views.take_invalidation();
        let mut domain_changed = causes.note(Cause::Sections, sections_changed) | !noted.is_empty();
        causes.extend(noted);
        domain_changed |= causes.note(Cause::Titlebar, self.shell.titlebar.take_changed());
        domain_changed |= causes.note(
            Cause::ViewEvents,
            self.views
                .take_events(&mut self.shell, self.ui.ui(), ctx, id),
        );
        if let Some(workspace) = self.shell.entry.workspace_mut() {
            domain_changed |= causes.note(
                Cause::CodeActions,
                workspace.drain_code_actions(&self.shell.core, &mut self.shell.failures, ctx, id),
            );
            domain_changed |= causes.note(
                Cause::QuestionSource,
                workspace.drain_question_source(&self.shell.core, &self.shell.dialogs, ctx, id),
            );
            domain_changed |= causes.note(
                Cause::Inspector,
                workspace.drain_inspector(
                    &mut self.shell.core,
                    &mut self.shell.failures,
                    &self.shell.dialogs,
                    self.ui.ui_mut(),
                    ctx,
                    id,
                ),
            );
            if workspace.inspector.reveal_tail(self.ui.ui_mut()) {
                ctx.request_frame(id, FrameReason::INPUT);
            }
        }
        drop(tm_workspace);
        let drain_span = crate::core::diagnostics::rebuild_stats::span("shell_drain");
        let tm_shell = Tm::new("drain.shell");
        let mut updates = self.shell.drain(active, self.ui.ui_mut());
        drop(tm_shell);
        drop(drain_span);
        let drained = std::mem::take(&mut updates.causes);
        domain_changed |= !drained.is_empty();
        causes.extend(drained);
        domain_changed |= causes.note(
            Cause::ComposerImage,
            self.views
                .open_composer_images(&mut self.shell, self.ui.ui_mut()),
        );
        if let Some(wait) = self.shell.live.remaining(self.shell.core.clock().now()) {
            ctx.request_animation_deadline(id, ctx.now() + wait);
        }
        aqua_core::checkpoint::reach();
        let tm_update = Tm::new("drain.update");
        domain_changed |= causes.note(Cause::WorkspaceActions, self.update(ctx, id, updates));
        drop(tm_update);
        aqua_core::checkpoint::reach();
        let tm_sync = Tm::new("drain.sync");
        if let Some(workspace) = self.shell.entry.workspace_mut() {
            workspace.sync_marketplace_ui(self.ui.ui_mut());
            let settings_visible =
                matches!(self.shell.core.screen(), crate::core::Screen::Settings(_));
            workspace.sync_editor_ui(self.ui.ui_mut(), settings_visible);
            if workspace.sync_composer_tooltip(
                self.ui.ui_mut(),
                &self.shell.preferences.theme(),
                self.shell.core.clock().now(),
            ) {
                ctx.request_frame(id, FrameReason::ANIMATION);
            }
            if let Some(deadline) = workspace
                .composer
                .as_ref()
                .and_then(editor_composer::view::ComposerOwner::tooltip_deadline)
            {
                let delay = deadline.saturating_duration_since(self.shell.core.clock().now());
                ctx.request_animation_deadline(id, ctx.now() + delay);
            }
        }
        drop(tm_sync);
        self.shell.bind(ctx, id);
        if !self.mounted {
            causes.note(Cause::Unmounted, true);
        }
        if (domain_changed || !self.mounted) && !self.refresh_drained(ctx, id, causes.refresh()) {
            self.rebuild(ctx, id, FrameReason::INPUT, &causes);
        }
    }
}

impl<V: SectionViews> Application for ShellApplication<V> {
    fn on_external_file_drag(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        event: &aqua::ExternalFileDrag,
    ) {
        self.ui.on_external_file_drag(ctx, id, event);
        self.drain(ctx, id, false);
    }

    fn input_invalidation_epoch(&self, id: AquaWindowId) -> u64 {
        self.ui.ui().input_invalidation_epoch(id)
    }

    fn color_profile(&self) -> aqua_core::ColorProfile {
        self.ui.color_profile()
    }
    fn color_profile_for(&self, id: AquaWindowId) -> aqua_core::ColorProfile {
        self.ui.color_profile_for(id)
    }
    fn on_click_settings(&mut self, settings: aqua_core::ClickSettings) {
        self.ui.on_click_settings(settings);
    }
    fn on_start(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) {
        self.ui.on_start(ctx, id);
        if let Some(window) = ctx.window_state(id) {
            let ui = self.ui.ui_mut();
            ui.set_viewport(
                crate::measure::narrow(window.inner_size.width),
                crate::measure::narrow(window.inner_size.height),
                crate::measure::narrow(window.scale_factor),
            );
            set_window_scale(ui, crate::measure::narrow(window.scale_factor));
        }
        self.shell.preferences.system =
            editor_ui::preferences::SystemAccess::from_os(ctx.system_preferences());
        self.shell.bind(ctx, id);
        if let Some(updates) = self.initial.take() {
            self.update(ctx, id, updates);
        }
        if self.fit_launch {
            fit_launch_to_display(ctx, id);
        }
        self.drain(ctx, id, true);
    }
    fn can_close(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) -> bool {
        self.ui.can_close(ctx, id)
    }
    fn on_wake(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) {
        let _tm = Tm::new("wake");
        let _span = crate::core::diagnostics::rebuild_stats::span("on_wake");
        self.ui.on_wake(ctx, id);
        let changed = self.views.wake(&mut self.shell, self.ui.ui_mut(), ctx, id);
        if self.views.glide_active() {
            ctx.request_frame(id, FrameReason::ANIMATION);
        }
        self.drain(ctx, id, changed);
    }
    fn on_animation_frame(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, _now: Instant) {
        let _tm = Tm::new("animation");
        let landed = self
            .shell
            .entry
            .workspace_mut()
            .is_some_and(|workspace| workspace.conversation.expire_landing());
        let animated = self
            .views
            .animate(&mut self.shell, self.ui.ui_mut(), ctx, id);
        let mut causes = self.views.take_invalidation();
        causes.note(Cause::LandingExpiry, landed);
        if animated | landed {
            let refresh = causes.refresh();
            if !self.refresh_sections(ctx, id, refresh) {
                self.rebuild(ctx, id, FrameReason::ANIMATION, &causes);
            }
        }
        self.advance_hover_animations(ctx, id);
        if self
            .shell
            .live
            .remaining(self.shell.core.clock().now())
            .is_some_and(|wait| wait.is_zero())
        {
            self.drain(ctx, id, false);
        }
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one frame-completion pass: notifications, qualification, refresh and pacing"
    )]
    fn on_frame_completed(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, sequence: u64) {
        #[cfg(feature = "qualification")]
        if self.notifications_start == NotificationStart::Disabled
            && let Err(error) =
                crate::qualification::observation::record_native_state(&self.shell, sequence)
        {
            tracing::warn!(
                error = %::diagnostics::chain(&error),
                "could not record qualification state"
            );
        }
        let _tm = Tm::new("fc");
        let _span = crate::core::diagnostics::rebuild_stats::span("on_fc");
        if self.start_notifications(ctx, id) {
            self.drain(ctx, id, false);
        }
        crate::core::diagnostics::open_timing::presented();
        self.ui.on_frame_completed(ctx, id, sequence);
        self.shell.bind(ctx, id);
        let views_changed =
            self.views
                .frame_completed(&mut self.shell, self.ui.ui_mut(), ctx, id, sequence);
        if self.views.glide_active() {
            ctx.request_frame(id, FrameReason::ANIMATION);
        }
        let viewport = ctx.window_state(id).map(|window| {
            aqua_core::Size::new(
                crate::measure::narrow(window.inner_size.width),
                crate::measure::narrow(window.inner_size.height),
            )
        });
        let theme = self.shell.preferences.theme();
        let images = self.shell.images().clone();
        if let Some(owner) = self
            .shell
            .entry
            .workspace_mut()
            .and_then(|workspace| workspace.composer.as_mut())
        {
            let identity = {
                let composer = owner.controller.borrow();
                (composer.picker.menu == Some(crate::composer::picker::Menu::Model))
                    .then(|| (composer.target.host().clone(), composer.host_instance()))
            };
            if let (Some(viewport), Some((host_id, Some(instance)))) = (viewport, identity)
                && let Some(host) = self.shell.core.host_if_instance(&host_id, instance)
            {
                match owner.model_menu(
                    &theme,
                    host,
                    viewport,
                    crate::shell::wheel::kept_offset(self.ui.ui(), crate::ui::scroll::MODEL_MENU).y,
                    &images,
                    self.shell.core.clock().now(),
                ) {
                    Ok(content) => {
                        if let Err(error) = owner.refresh_popup_content(
                            crate::composer::view::PopupKind::Model,
                            self.ui.ui_mut(),
                            content,
                            ctx.now().as_secs_f64(),
                        ) {
                            tracing::warn!(%error, "could not refresh composer model popup");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(%error, "could not build composer model popup");
                    }
                }
            }
        }
        let popup_animating = self
            .shell
            .entry
            .workspace()
            .and_then(|workspace| workspace.composer.as_ref())
            .is_some_and(|owner| owner.needs_animation(self.ui.ui()));
        if popup_animating {
            ctx.request_frame(id, FrameReason::ANIMATION);
        }
        self.advance_hover_animations(ctx, id);
        let usage_elapsed = self
            .usage_deadline
            .is_some_and(|deadline| ctx.now() >= deadline);
        let mut causes = Invalidation::default();
        let noted = self.views.take_invalidation();
        let mut refresh = causes.note(Cause::ViewsFrame, views_changed) | !noted.is_empty();
        causes.extend(noted);
        refresh |= causes.note(Cause::PopupAnimating, popup_animating);
        refresh |= causes.note(Cause::UsageClock, usage_elapsed);
        if usage_elapsed {
            self.usage_deadline =
                Some(ctx.now() + crate::sidebar::usage::controller::CLOCK_INTERVAL);
        }
        if let Some(workspace) = self.shell.entry.workspace_mut() {
            if workspace.sync_composer_tooltip(
                self.ui.ui_mut(),
                &theme,
                self.shell.core.clock().now(),
            ) {
                ctx.request_frame(id, FrameReason::ANIMATION);
            }
            refresh |= causes.note(
                Cause::CodeActions,
                workspace.drain_code_actions(&self.shell.core, &mut self.shell.failures, ctx, id),
            );
            refresh |= causes.note(Cause::CodeFrame, workspace.take_code_frame(ctx.now()));
            if workspace
                .markdown
                .borrow()
                .reveal_heatmaps(self.ui.ui_mut())
            {
                ctx.request_frame(id, FrameReason::ANIMATION);
            }
            refresh |= causes.note(Cause::InspectorTick, workspace.inspector.tick(ctx.now()));
            let reduced = self.ui.ui().host().motion().reduced_motion()
                == aqua_ui::motion::ReducedMotion::Reduce;
            let now = self.shell.core.clock().now();
            let sidebar_moving = workspace.sidebar.advance_motion(now, reduced)
                | workspace.settings_sidebar.advance_motion(now, reduced);
            if sidebar_moving {
                ctx.request_frame(id, FrameReason::ANIMATION);
                refresh = causes.note(Cause::SidebarMotion, true);
            }
            if workspace
                .sidebar
                .next_tick()
                .is_some_and(|at| at <= self.shell.core.clock().timestamp())
            {
                workspace
                    .sidebar
                    .sync(&self.shell.core, self.shell.core.clock().timestamp());
                refresh = causes.note(Cause::SidebarTick, true);
            }
            refresh |= causes.note(
                Cause::LandingExpiry,
                workspace.conversation.expire_landing(),
            );
            if workspace.marketplace.needs_animation() {
                crate::shell::wheel::repaint(
                    self.ui.ui_mut(),
                    crate::marketplace::view::LOADING_KEY,
                );
                ctx.request_frame(id, FrameReason::ANIMATION);
            }
        }
        if refresh && !self.refresh_sections(ctx, id, causes.refresh()) {
            self.rebuild(ctx, id, FrameReason::ANIMATION, &causes);
        }
    }
    fn cursor(&self) -> Option<CursorIcon> {
        Some(self.ui.ui().cursor())
    }
    fn cursor_for(&self, id: AquaWindowId) -> Option<CursorIcon> {
        self.ui.cursor_for(id)
    }
    fn step_input(&mut self, id: AquaWindowId, input: aqua::RuntimeInput) {
        let _tm = Tm::new("step");
        self.ui.step_input(id, input);
    }
    fn take_output(&mut self, id: AquaWindowId) -> aqua::RuntimeOutput {
        let _tm = Tm::new("take");
        let output = self.ui.take_output(id);
        #[cfg(feature = "capture")]
        if let Some(tap) = &self.render_tap {
            tap.borrow_mut().extend(output.render.iter().cloned());
        }
        output
    }
    fn on_dialog(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        request: aqua_protocol::RequestId,
        outcome: aqua_protocol::DialogOutcome,
    ) {
        if !self.shell.dialogs.complete(request, outcome.clone()) {
            self.ui.on_dialog(ctx, id, request, outcome);
        }
        self.drain(ctx, id, false);
    }
    fn on_window_event(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, event: &WindowEvent) {
        let _tm = Tm::new("winev");
        self.ui.on_window_event(ctx, id, event);
        if matches!(event, WindowEvent::Destroyed(..)) {
            self.shell.dialogs.clear();
        }
        if let WindowEvent::ScaleChanged(_, scale) = event {
            set_window_scale(self.ui.ui_mut(), crate::measure::narrow(*scale));
        }
        match event {
            WindowEvent::Resized(..) => {
                let rebuild = !self.mounted
                    || ctx
                        .window_state(id)
                        .is_none_or(|window| self.views.resize_needs_rebuild(&self.shell, window));
                self.drain(ctx, id, rebuild);
            }
            WindowEvent::ScaleChanged(..) => self.drain(ctx, id, true),
            WindowEvent::ThemeChanged(..) => {
                let system =
                    editor_ui::preferences::SystemAccess::from_os(ctx.system_preferences());
                if self.shell.preferences.system != system {
                    self.shell.preferences.system = system;
                    self.apply_motion_policy(ctx);
                    self.drain(ctx, id, true);
                }
            }
            WindowEvent::Focused(_, focused) => {
                self.drain(ctx, id, true);
                if *focused {
                    ctx.request_frame(id, FrameReason::ANIMATION);
                }
            }
            WindowEvent::CloseRequested(_)
            | WindowEvent::Occluded(..)
            | WindowEvent::RedrawRequested(_)
            | WindowEvent::Destroyed(_) => {}
        }
    }
    fn on_input(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, event: &InputEvent) {
        let _tm = Tm::new("input");
        for popup in [
            &mut self.font_popup,
            &mut self.streaming_popup,
            &mut self.text_size_popup,
            &mut self.contrast_popup,
            &mut self.retention_popup,
        ]
        .into_iter()
        .flatten()
        {
            match popup.request_dismissal(self.ui.ui_mut(), event) {
                Ok(true) => {
                    ctx.request_frame(id, FrameReason::ANIMATION);
                    return;
                }
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "could not dismiss settings popup"),
            }
        }
        let dismissed = {
            let _tm = Tm::new("input.dismiss");
            self.shell
                .entry
                .workspace_mut()
                .and_then(|workspace| workspace.composer.as_mut())
                .is_some_and(|owner| {
                    match owner.dismiss_popups_for_event(self.ui.ui_mut(), event) {
                        Ok(consumed) => consumed,
                        Err(error) => {
                            tracing::warn!(%error, "could not dismiss composer popup");
                            false
                        }
                    }
                })
        };
        if dismissed {
            ctx.request_frame(id, FrameReason::ANIMATION);
            self.drain(ctx, id, false);
            return;
        }
        let mut glided = false;
        let consumed = {
            let _tm = Tm::new("input.route");
            let pre = {
                let _tm = Tm::new("input.pre");
                self.views
                    .pre_input(&self.shell, self.ui.ui_mut(), event, ctx.now())
            };
            if let Some(consumed) = pre {
                glided = matches!(event, InputEvent::Scroll { .. });
                consumed
            } else {
                let _tm = Tm::new("input.ui");
                self.ui.on_input(ctx, id, event);
                self.ui.ui().last_input_consumed()
            }
        };
        let terminal_changed = {
            let _tm = Tm::new("input.terminal");
            self.shell
                .entry
                .workspace_mut()
                .is_some_and(|workspace| workspace.sync_terminal_input(self.ui.ui_mut()))
        };
        let changed = terminal_changed | {
            let _tm = Tm::new("input.views");
            self.views
                .input(&mut self.shell, self.ui.ui_mut(), ctx, id, event, consumed)
        };
        if glided && !changed {
            return;
        }
        let _tm_drain = Tm::new("input.drain");
        self.drain(ctx, id, changed);
        self.advance_hover_animations(ctx, id);
    }
    fn on_files_dropped_at(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        paths: &[std::path::PathBuf],
        at: Option<aqua_core::Point>,
    ) {
        self.ui.on_files_dropped_at(ctx, id, paths, at);
        self.drain(ctx, id, false);
    }
    fn on_a11y_action(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        request: &A11yActionRequest,
    ) {
        let moves = moves_the_transcript(self.ui.ui(), request);
        self.ui.on_a11y_action(ctx, id, request);
        if moves {
            self.views.reader_moved_transcript();
        }
        self.drain(ctx, id, false);
    }
    fn prepare_accessibility(
        &mut self,
        window: &ObservedWindowState,
        now: Instant,
    ) -> Option<Instant> {
        self.ui.prepare_accessibility(window, now)
    }
    fn a11y_nodes(&self) -> Option<(NodeId, NodeId, Vec<A11yNode>)> {
        self.ui.a11y_nodes()
    }
    fn a11y_nodes_for(&self, id: AquaWindowId) -> Option<(NodeId, NodeId, Vec<A11yNode>)> {
        self.ui.a11y_nodes_for(id)
    }
    fn a11y_revision_for(&self, id: AquaWindowId) -> Option<u64> {
        self.ui.ui().a11y_revision_for(id)
    }
    fn chrome_regions(&self, window: &ObservedWindowState) -> Option<DragRegions> {
        self.shell
            .titlebar
            .regions()
            .or_else(|| self.ui.chrome_regions(window))
    }
    fn externals(&mut self, frame: &mut ExternalFrame<'_>) {
        let _tm = Tm::new("ext");
        self.views
            .advance_glides(&mut self.shell, self.ui.ui_mut(), frame.now);
        let supported = Some(false);
        if self.shell.meshes_supported.replace(supported) != supported {
            self.shell.meshes_changed.set(true);
            let _queued = self.shell.core.wake_mailbox().sender().send(());
        }
        if self
            .shell
            .live
            .remaining(self.shell.core.clock().now())
            .is_some_and(|wait| wait.is_zero())
        {
            let _queued = self.shell.core.wake_mailbox().sender().send(());
        }
        crate::core::diagnostics::open_timing::externals_begin();
        let started = crate::shell::stopwatch::Stopwatch::start();
        self.ui.externals(frame);
        crate::core::diagnostics::open_timing::scene(started.measured());
    }
    fn build_scene(&mut self, window: &ObservedWindowState, now: Instant) -> Option<Instant> {
        let _tm = Tm::new("scene");
        self.ui.build_scene(window, now)
    }
    fn scene_mut(&mut self, id: AquaWindowId) -> Option<&mut Scene> {
        self.ui.scene_mut(id)
    }
    fn reactive_runtime(&self) -> Option<&aqua_reactive::Runtime> {
        self.ui.reactive_runtime()
    }
    fn reactive_runtime_for(&self, id: AquaWindowId) -> Option<&aqua_reactive::Runtime> {
        self.ui.reactive_runtime_for(id)
    }
}

fn moves_the_transcript(ui: &UiApp, request: &A11yActionRequest) -> bool {
    use aqua_protocol::A11yAction;
    let Some(transcript) = crate::shell::wheel::keyed_node(ui, "conversation-transcript") else {
        return false;
    };
    match request.action {
        A11yAction::Scroll(_) | A11yAction::SetScrollOffset => request.node == transcript,
        A11yAction::ScrollIntoView => {
            std::iter::successors(Some(request.node), |node| ui.tree().get(*node)?.parent)
                .any(|node| node == transcript)
        }
        A11yAction::Press
        | A11yAction::Focus
        | A11yAction::SetValue
        | A11yAction::ReplaceSelectedText
        | A11yAction::Expand
        | A11yAction::Collapse
        | A11yAction::Increment
        | A11yAction::Decrement
        | A11yAction::ShowContextMenu
        | A11yAction::Other(_) => false,
    }
}

fn close_popup(ui: &mut UiApp, slot: &mut Option<crate::ui::popover::AnimatedPickerHost>) {
    if let Some(popup) = slot.take() {
        popup.dispose(ui);
    }
}

fn sync_popup(
    ui: &mut UiApp,
    slot: &mut Option<crate::ui::popover::AnimatedPickerHost>,
    key: &str,
    content: Option<(Element, aqua_ui::popup::PopupOptions)>,
    now: f64,
) {
    if slot.as_ref().is_some_and(|popup| !popup.is_open(ui)) {
        *slot = None;
    }
    let Some((element, options)) = content else {
        if let Some(popup) = slot.as_mut().filter(|popup| !popup.is_closing())
            && let Err(error) = popup.begin_close(ui, now)
        {
            tracing::warn!(%error, "could not close settings popup");
        }
        return;
    };
    if let Some(popup) = slot.as_mut() {
        if let Err(error) = popup.refresh_content(ui, element) {
            tracing::warn!(%error, "could not refresh settings popup");
        }
        return;
    }
    let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
    let mut nodes = ui.tree().nodes_with_key(hash).into_iter();
    let Some(owner) = nodes.next() else {
        return;
    };
    if nodes.next().is_some() {
        return;
    }
    match crate::ui::popover::AnimatedPickerHost::open_options(ui, owner, element, options, now) {
        Ok(popup) => *slot = Some(popup),
        Err(error) => tracing::warn!(%error, "could not open settings popup"),
    }
}

fn fit_launch_to_display(ctx: &mut AppCtx<'_>, id: AquaWindowId) {
    let Some(desired) = ctx.window_state(id).map(|window| window.inner_size) else {
        return;
    };
    let mut minimum = desired;
    minimum.width = f64::from(crate::shell::layout::MIN_WINDOW_WIDTH);
    minimum.height = f64::from(crate::shell::layout::MIN_WINDOW_HEIGHT);
    let Some(fitted) = ctx
        .display(id)
        .and_then(|display| display.fit_window(desired, minimum))
    else {
        return;
    };
    if fitted == desired {
        return;
    }
    let requested = ctx
        .request_window(aqua::prelude::WindowRequest::SetInnerSize(id, fitted))
        .and_then(|()| ctx.request_window(aqua::prelude::WindowRequest::Center(id)));
    if let Err(error) = requested {
        tracing::warn!(%error, "could not fit the launch window to the display");
    }
}

fn failure_card(problem: &crate::core::Problem, theme: &crate::ui::theme::ArtisanTheme) -> Element {
    use crate::ui::kit::pane::sheet;
    use aqua_ui::element::Box as UiBox;
    use aqua_ui::style::{Length, Style};
    const WIDTH: f32 = 480.0;
    let card = crate::ui::kit::ProblemNotice::new("view-failure-problem", *theme, problem)
        .build()
        .style(Style::new().width(Length::Px(WIDTH)));
    window_root(
        UiBox::new()
            .width(Length::Pct(100.0))
            .height(Length::Pct(100.0))
            .layout("column")
            .style(
                Style::new()
                    .align(aqua_ui::AlignItems::CENTER)
                    .justify(aqua_ui::JustifyContent::CENTER),
            )
            .style(sheet(theme))
            .child(card)
            .build(),
    )
}

fn window_root(element: Element) -> Element {
    element
        .role(aqua_core::Role::Group)
        .aria_label(crate::shell::host::WINDOW_NAME)
        .aria_language(crate::shell::host::INTERFACE_LANGUAGE)
}

pub(crate) struct Tm {
    name: &'static str,
    started: crate::shell::stopwatch::Stopwatch,
    heap: aqua::profile::alloc::HeapCounts,
    _span: crate::core::diagnostics::rebuild_stats::Span,
}
const DEFAULT_THRESHOLD_MS: f64 = 3.0;

impl Tm {
    fn threshold_ms() -> f64 {
        static VALUE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
        *VALUE.get_or_init(|| match std::env::var("AQUA_TM_MS") {
            Err(std::env::VarError::NotPresent) => DEFAULT_THRESHOLD_MS,
            Err(error) => {
                tracing::warn!(%error, "AQUA_TM_MS is unreadable; timing spans over 3 ms");
                DEFAULT_THRESHOLD_MS
            }
            Ok(value) => value.parse().unwrap_or_else(|error| {
                tracing::warn!(%error, value, "AQUA_TM_MS is not a number; timing spans over 3 ms");
                DEFAULT_THRESHOLD_MS
            }),
        })
    }

    pub(crate) fn new(name: &'static str) -> Self {
        crate::core::diagnostics::rebuild_stats::record_event(name);
        Self {
            name,
            started: crate::shell::stopwatch::Stopwatch::start(),
            heap: aqua::profile::alloc::snapshot(),
            _span: crate::core::diagnostics::rebuild_stats::span(name),
        }
    }
}
impl Drop for Tm {
    fn drop(&mut self) {
        let ms = self.started.measured().as_secs_f64() * 1000.0;
        if aqua::profile::enabled() {
            let heap = aqua::profile::alloc::snapshot().since(self.heap);
            if ms > 0.25 || heap.allocs > 8 {
                let name = self.name;
                aqua::profile::note(|| format!("{name}={ms:.2}ms/a{}", heap.allocs));
            }
        }
        if ms > Self::threshold_ms() {
            tracing::info!(target: "tm", what = self.name, ms);
        }
    }
}

#[cfg(all(test, feature = "qualification"))]
#[path = "../../tests/qualification/native.rs"]
mod native_tests;

pub(crate) fn begin_theme_transition(
    ctx: &mut AppCtx<'_>,
    id: AquaWindowId,
    dark: bool,
    reduced: bool,
) {
    if reduced {
        return;
    }
    let mode = if dark {
        crate::ui::theme::ThemeMode::Dark
    } else {
        crate::ui::theme::ThemeMode::Light
    };
    let shimmer = crate::ui::theme::ArtisanTheme::for_mode(mode)
        .colors
        .link
        .fade(0.45)
        .to_reference_text_color();
    let transition = aqua_core::scene::GpuiSourceSnapshotTransition {
        duration: std::time::Duration::from_millis(700),
        band: 0.18,
        shimmer,
    };
    if let Err(error) = ctx.request_window(
        aqua::prelude::WindowRequest::BeginGpuiSourceSnapshotTransition(id, transition),
    ) {
        tracing::warn!(%error, "could not start theme transition");
    }
}
