use crate::shell::host::{Shell, ShellResources, ShellUpdates, set_window_scale};
use crate::shell::native_notifications::NativeNotifications;
use aqua::prelude::{
    A11yActionRequest, A11yNode, AquaWindowId, CursorIcon, DragRegions, FrameReason, InputEvent,
    Instant, ManagedUiApp, NodeId, ObservedWindowState, Scene, WindowEvent,
};
use aqua::{AppCtx, Application, ExternalFrame, UiApp};
use aqua_ui::element::Element;

pub trait SectionViews {
    fn advance_glides(&mut self, _shell: &mut Shell, _ui: &mut UiApp) -> bool {
        false
    }
    fn glide_active(&self) -> bool {
        false
    }
    fn pre_input(&mut self, _ui: &mut UiApp, _input: &InputEvent) -> Option<bool> {
        None
    }
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
    ) -> anyhow::Result<Element>;
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

pub struct ShellApplication<V> {
    pub shell: Shell,
    pub views: V,
    ui: ManagedUiApp,
    notifications: Option<NativeNotifications>,
    notifications_started: bool,
    notifications_enabled: bool,
    initial: Option<ShellUpdates>,
    mounted: bool,
    font_popup: Option<aqua_ui::overlay::OverlayId>,
    streaming_popup: Option<aqua_ui::overlay::OverlayId>,
    retention_popup: Option<aqua_ui::overlay::OverlayId>,
    usage_deadline: Option<Instant>,
    fit_launch: bool,
    render_tap: Option<std::rc::Rc<std::cell::RefCell<Vec<aqua_protocol::RenderCommand>>>>,
}

#[cfg(feature = "qualification")]
impl ShellApplication<crate::shell::root::RootViews> {
    pub fn arm_qualification_paint(&mut self, generation: u64) -> bool {
        self.views.arm_qualification_paint(&self.shell, generation)
    }

    pub fn qualification_paint(&self) -> Option<crate::qualification::observation::Paint> {
        self.views.qualification_paint(&self.shell)
    }

    pub fn qualification_observation(
        &self,
    ) -> Option<crate::qualification::observation::Observation> {
        self.views
            .qualification_observation(&self.shell, self.ui.ui())
    }
}

impl<V: SectionViews> ShellApplication<V> {
    pub fn new(files: crate::core::EditorFiles, resources: ShellResources, views: V) -> Self {
        let (shell, initial) = Shell::new(files, resources.images.clone());
        Self::from_shell(shell, initial, resources, views)
    }

    pub fn opening(
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
        app.notifications_enabled = false;
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
            notifications_started: false,
            notifications_enabled: true,
            initial: Some(initial),
            mounted: false,
            font_popup: None,
            streaming_popup: None,
            retention_popup: None,
            usage_deadline: None,
            fit_launch: false,
            render_tap: None,
        }
    }

    pub fn tap_render(
        &mut self,
    ) -> std::rc::Rc<std::cell::RefCell<Vec<aqua_protocol::RenderCommand>>> {
        self.render_tap.get_or_insert_with(Default::default).clone()
    }

    pub fn fitting_launch_to_display(mut self) -> Self {
        self.fit_launch = true;
        self
    }

    fn start_notifications(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId) -> bool {
        if !self.notifications_enabled || std::mem::replace(&mut self.notifications_started, true) {
            return false;
        }
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

    fn update(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, updates: ShellUpdates) {
        for effect in updates.settings_platform_effects {
            match effect {
                crate::shell::workspace::SettingsPlatformEffect::OpenUrl(url) => {
                    if let Err(error) = crate::shell::open_url(&url) {
                        tracing::warn!(%error, "could not open URL");
                    }
                }
                crate::shell::workspace::SettingsPlatformEffect::CopyText(text) => {
                    if let Err(error) = ctx.clipboard().set_text(text) {
                        tracing::warn!(%error, "could not copy text");
                    }
                }
            }
        }
        if let Some(dark) = updates.theme_transition {
            if self.ui.ui().host().motion().reduced_motion()
                != aqua_ui::motion::ReducedMotion::Reduce
            {
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
        }
        if updates.preferences_changed {
            if let Some(reduced) = self.shell.preferences.reduced_motion {
                let policy = if reduced {
                    aqua_ui::motion::ReducedMotion::Reduce
                } else {
                    aqua_ui::motion::ReducedMotion::Full
                };
                self.ui
                    .ui_mut()
                    .host_mut()
                    .motion_mut()
                    .set_reduced_motion(policy, ctx.now().as_secs_f64());
            }
            if let Err(error) = self.shell.apply_preferences(ctx, id) {
                tracing::error!(%error, "could not apply window preferences");
            }
        }
        if !updates.notifications.is_empty() {
            self.start_notifications(ctx, id);
        }
        if let Some(center) = &mut self.notifications {
            for effects in updates.notifications {
                if let Err(error) = center.apply(effects, &self.shell, ctx, id) {
                    tracing::warn!(%error, "could not apply notification effects");
                }
            }
        }
    }

    fn rebuild(
        &mut self,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        reason: FrameReason,
        causes: &[&'static str],
    ) {
        let _tm = Tm::new("rebuild");
        let _rebuild_span = crate::shell::rebuild_stats::span("rb_total");
        self.shell.live.flushed(std::time::Instant::now());
        crate::shell::rebuild_stats::record_rebuild(causes);
        let Some(window) = ctx.window_state(id).cloned() else {
            return;
        };
        let was_mounted = self.mounted;
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
        let build_span = crate::shell::rebuild_stats::span("rb_build");
        let built = self.views.build(&mut self.shell, self.ui.ui_mut(), &window);
        drop(build_span);
        aqua_core::checkpoint::reach();
        let element = match built {
            Ok(element) => element,
            Err(error) => {
                tracing::error!(%error, "could not build Editor view");
                self.show_failure(&error.to_string(), ctx, id, reason);
                self.shell.bind(ctx, id);
                return;
            }
        };
        let refresh_span = crate::shell::rebuild_stats::span("rb_refresh");
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
                let mounted_span = crate::shell::rebuild_stats::span("rb_mounted");
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
                        .sync_terminal_dialog(self.ui.ui_mut(), self.shell.preferences.theme());
                    workspace.inspector.reveal_tail(self.ui.ui_mut());
                    if workspace.sync_composer_popups(
                        &self.shell.core,
                        self.ui.ui_mut(),
                        self.shell.preferences.theme(),
                        &assets,
                        ctx.now().as_secs_f64(),
                    ) {
                        ctx.request_frame(id, FrameReason::ANIMATION);
                    }
                }
                aqua_core::checkpoint::reach();
                self.sync_settings_popups();
                if matches!(
                    self.shell.core.workspace_screen(),
                    crate::core::Screen::Usage
                ) {
                    let deadline = *self.usage_deadline.get_or_insert_with(|| {
                        ctx.now() + crate::sidebar::usage_controller::CLOCK_INTERVAL
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
                        u64::try_from(at.saturating_sub(artisan_api::now())).unwrap_or(0),
                    );
                    ctx.request_animation_deadline(id, ctx.now() + delay);
                }
                if let Some(deadline) = self
                    .shell
                    .entry
                    .workspace()
                    .and_then(|workspace| workspace.conversation.landing)
                {
                    let delay = deadline.saturating_duration_since(std::time::Instant::now());
                    ctx.request_animation_deadline(id, ctx.now() + delay);
                }
                if !was_mounted || self.ui.ui().tree().invalidation_epoch() != epoch {
                    ctx.request_frame(id, reason);
                }
            }
            Err(error) => {
                tracing::error!(%error, "could not mount Editor view");
                self.show_failure(&error.to_string(), ctx, id, reason);
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
        crate::shell::rebuild_stats::record_event("transcript_subtree");
        self.shell.live.flushed(std::time::Instant::now());
        self.views
            .mounted(&mut self.shell, self.ui.ui_mut(), ctx, id);
        if self.ui.ui().tree().invalidation_epoch() != epoch {
            ctx.request_frame(id, reason);
        }
        self.shell.bind(ctx, id);
        true
    }

    fn show_failure(
        &mut self,
        message: &str,
        ctx: &mut AppCtx<'_>,
        id: AquaWindowId,
        reason: FrameReason,
    ) {
        let card = failure_card(message, self.shell.preferences.theme());
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

    fn sync_settings_popups(&mut self) {
        let Some(workspace) = self.shell.entry.workspace() else {
            close_popup(self.ui.ui_mut(), &mut self.font_popup);
            close_popup(self.ui.ui_mut(), &mut self.streaming_popup);
            close_popup(self.ui.ui_mut(), &mut self.retention_popup);
            return;
        };
        let settings_open = matches!(self.shell.core.screen(), crate::core::Screen::Settings(_));
        let font_open = settings_open && workspace.settings.font_select.borrow().open;
        let streaming_open = settings_open && workspace.settings.streaming_select.borrow().open;
        let retention_open = settings_open && workspace.settings.retention_select.borrow().open;
        let theme = self.shell.preferences.theme();
        let font = font_open.then(|| {
            crate::settings::view::font_menu(
                &workspace.settings,
                &self.shell.core,
                theme,
                workspace.settings_emit(),
            )
            .into_parts()
        });
        let streaming = streaming_open.then(|| {
            crate::settings::view::streaming_menu(
                &workspace.settings,
                &self.shell.core,
                theme,
                workspace.settings_emit(),
            )
            .into_parts()
        });
        let retention = if retention_open {
            self.shell.core.active_host().map(|host| {
                crate::settings::archive_view::retention_menu(
                    &workspace.settings,
                    host,
                    theme,
                    workspace.settings_emit(),
                )
                .into_parts()
            })
        } else {
            None
        };
        sync_popup(
            self.ui.ui_mut(),
            &mut self.font_popup,
            "settings-font-set",
            font,
        );
        sync_popup(
            self.ui.ui_mut(),
            &mut self.streaming_popup,
            "settings-streaming-mode",
            streaming,
        );
        sync_popup(
            self.ui.ui_mut(),
            &mut self.retention_popup,
            "settings-thread-retention",
            retention,
        );
    }

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
        if let Some(notifications) = &mut self.notifications {
            if let Err(error) = notifications.drain(&mut self.shell, ctx, id) {
                tracing::warn!(%error, "could not activate notification");
            }
        }
        drop(tm_pre);
        let mut causes: Vec<&'static str> = Vec::new();
        let tm_workspace = Tm::new("drain.workspace");
        let mut domain_changed = mark(&mut causes, "sections", sections_changed);
        if sections_changed {
            causes.extend(crate::shell::rebuild_stats::take_notes());
        }
        domain_changed |= mark(&mut causes, "titlebar", self.shell.titlebar.take_changed());
        if let Some(workspace) = self.shell.entry.workspace_mut() {
            domain_changed |= mark(
                &mut causes,
                "code_actions",
                workspace.drain_code_actions(&self.shell.core, ctx, id),
            );
            domain_changed |= mark(
                &mut causes,
                "question_source",
                workspace.drain_question_source(&self.shell.core, &self.shell.dialogs, ctx, id),
            );
            domain_changed |= mark(
                &mut causes,
                "inspector",
                workspace.drain_inspector(
                    &mut self.shell.core,
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
        let drain_span = crate::shell::rebuild_stats::span("shell_drain");
        let tm_shell = Tm::new("drain.shell");
        let updates = self.shell.drain(active, self.ui.ui_mut());
        drop(tm_shell);
        drop(drain_span);
        domain_changed |= mark(&mut causes, "shell_updates", updates.changed);
        let live_only = updates.live_only;
        if let Some(wait) = self.shell.live.remaining(std::time::Instant::now()) {
            ctx.request_animation_deadline(id, ctx.now() + wait);
        }
        aqua_core::checkpoint::reach();
        let tm_update = Tm::new("drain.update");
        self.update(ctx, id, updates);
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
                self.shell.preferences.theme(),
                std::time::Instant::now(),
            ) {
                ctx.request_frame(id, FrameReason::ANIMATION);
            }
            if let Some(deadline) = workspace
                .composer
                .as_ref()
                .and_then(|owner| owner.tooltip_deadline())
            {
                let delay = deadline.saturating_duration_since(std::time::Instant::now());
                ctx.request_animation_deadline(id, ctx.now() + delay);
            }
        }
        drop(tm_sync);
        self.shell.bind(ctx, id);
        if !self.mounted {
            causes.push("unmounted");
        }
        if domain_changed || !self.mounted {
            let transcript_only = live_only && self.mounted && causes.as_slice() == ["shell_updates"];
            if !(transcript_only && self.refresh_transcript(ctx, id, FrameReason::INPUT)) {
                self.rebuild(ctx, id, FrameReason::INPUT, &causes);
            }
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
                window.inner_size.width as f32,
                window.inner_size.height as f32,
                window.scale_factor as f32,
            );
            set_window_scale(ui, window.scale_factor as f32);
        }
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
        let _span = crate::shell::rebuild_stats::span("on_wake");
        self.ui.on_wake(ctx, id);
        let changed = self.views.wake(&mut self.shell, self.ui.ui_mut(), ctx, id);
        if self.views.glide_active() {
            ctx.request_frame(id, FrameReason::ANIMATION);
        }
        self.drain(ctx, id, changed);
    }
    fn on_frame_completed(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, sequence: u64) {
        #[cfg(feature = "qualification")]
        if !self.notifications_enabled {
            if let Err(error) =
                crate::qualification::observation::record_native_state(&self.shell, sequence)
            {
                tracing::warn!(%error, "could not record qualification state");
            }
        }
        let _tm = Tm::new("fc");
        let _span = crate::shell::rebuild_stats::span("on_fc");
        if self.start_notifications(ctx, id) {
            self.drain(ctx, id, false);
        }
        crate::conversation::open_timing::presented();
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
                window.inner_size.width as f32,
                window.inner_size.height as f32,
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
            if let Err(error) = owner.advance_popups(self.ui.ui_mut(), ctx.now().as_secs_f64()) {
                tracing::warn!(%error, "could not advance composer popup");
            }
            owner.advance_note_ticker(self.ui.ui_mut(), ctx.now().as_secs_f64());
            let identity = {
                let composer = owner.controller.borrow();
                (composer.picker.menu == Some(crate::composer::picker::Menu::Model))
                    .then(|| (composer.target.host().clone(), composer.host_instance()))
            };
            if let (Some(viewport), Some((host_id, Some(instance)))) = (viewport, identity) {
                if let Some(host) = self.shell.core.host_if_instance(&host_id, instance) {
                    match owner.model_menu(
                        theme,
                        host,
                        viewport,
                        &images,
                        std::time::Instant::now(),
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
                            tracing::warn!(%error, "could not build composer model popup")
                        }
                    }
                }
            }
            let roots = [
                crate::composer::view::PopupKind::Model,
                crate::composer::view::PopupKind::Project,
                crate::composer::view::PopupKind::Axis,
            ]
            .map(|kind| owner.popup_root(kind, self.ui.ui()));
            match owner.advance_picker_hover(
                self.ui.ui_mut(),
                &theme,
                ctx.now().as_secs_f64(),
                roots,
            ) {
                Ok(true) => ctx.request_frame(id, FrameReason::ANIMATION),
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "could not advance composer picker hover"),
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
        let hover_surface = {
            let ui = self.ui.ui();
            let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
                "sidebar-threads-hover-surface",
            ));
            ui.tree().sole_node_with_key(key)
        };
        if let (Some(surface), Some(workspace)) = (hover_surface, self.shell.entry.workspace_mut())
        {
            let reduced = self.ui.ui().host().motion().reduced_motion()
                == aqua_ui::motion::ReducedMotion::Reduce;
            match workspace.sidebar.advance_threads_hover(
                self.ui.ui_mut(),
                surface,
                &theme,
                ctx.now().as_secs_f64(),
                reduced,
            ) {
                Ok(frame) if frame.changed || frame.animating => {
                    ctx.request_frame(id, FrameReason::ANIMATION)
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(%error, "could not advance sidebar hover"),
            }
        }
        let navigator_surface = keyed_node(self.ui.ui(), "turn-navigator-hover-surface");
        if let (Some(surface), Some(workspace)) =
            (navigator_surface, self.shell.entry.workspace_mut())
        {
            let reduced = self.ui.ui().host().motion().reduced_motion()
                == aqua_ui::motion::ReducedMotion::Reduce;
            match workspace.navigator.advance_hover_frame(
                self.ui.ui_mut(),
                surface,
                &theme,
                ctx.now().as_secs_f64(),
                reduced,
            ) {
                Ok(frame) => {
                    if frame.changed || frame.animating {
                        ctx.request_frame(id, FrameReason::ANIMATION);
                    }
                }
                Err(error) => tracing::warn!(%error, "could not advance navigator hover"),
            }
        }
        let usage_elapsed = self
            .usage_deadline
            .is_some_and(|deadline| ctx.now() >= deadline);
        let mut causes: Vec<&'static str> = Vec::new();
        let notes = crate::shell::rebuild_stats::take_notes();
        let mut refresh = mark(&mut causes, "views_frame", views_changed);
        if views_changed {
            causes.extend(notes);
        }
        refresh |= mark(&mut causes, "popup_animating", popup_animating);
        refresh |= mark(&mut causes, "usage_clock", usage_elapsed);
        if usage_elapsed {
            self.usage_deadline =
                Some(ctx.now() + crate::sidebar::usage_controller::CLOCK_INTERVAL);
        }
        if let Some(workspace) = self.shell.entry.workspace_mut() {
            if workspace.sync_composer_tooltip(self.ui.ui_mut(), theme, std::time::Instant::now()) {
                ctx.request_frame(id, FrameReason::ANIMATION);
            }
            refresh |= mark(
                &mut causes,
                "code_actions",
                workspace.drain_code_actions(&self.shell.core, ctx, id),
            );
            refresh |= mark(&mut causes, "code_frame", workspace.take_code_frame(ctx.now()));
            if workspace
                .markdown
                .borrow()
                .reveal_heatmaps(self.ui.ui_mut())
            {
                ctx.request_frame(id, FrameReason::ANIMATION);
            }
            refresh |= mark(&mut causes, "inspector_tick", workspace.inspector.tick(ctx.now()));
            let reduced = self.ui.ui().host().motion().reduced_motion()
                == aqua_ui::motion::ReducedMotion::Reduce;
            let now = std::time::Instant::now();
            let sidebar_moving = workspace.sidebar.advance_motion(now, reduced)
                | workspace.settings_sidebar.advance_motion(now, reduced);
            if sidebar_moving {
                ctx.request_frame(id, FrameReason::ANIMATION);
                refresh = mark(&mut causes, "sidebar_motion", true);
            }
            if workspace
                .sidebar
                .next_tick()
                .is_some_and(|at| at <= artisan_api::now())
            {
                workspace.sidebar.sync(&self.shell.core, artisan_api::now());
                refresh = mark(&mut causes, "sidebar_tick", true);
            }
            refresh |= mark(
                &mut causes,
                "landing_expiry",
                workspace
                    .conversation
                    .expire_landing(std::time::Instant::now()),
            );
            refresh |= mark(
                &mut causes,
                "marketplace_animation",
                workspace.marketplace.needs_animation(),
            );
        }
        let sidebar_only = refresh
            && causes
                .iter()
                .all(|cause| matches!(*cause, "sidebar_motion" | "sidebar_tick"));
        let sidebar_refreshed = sidebar_only
            && ctx
                .window_state(id)
                .cloned()
                .is_some_and(|window| {
                    self.views
                        .refresh_sidebar(&self.shell, self.ui.ui_mut(), &window)
                });
        let transcript_only = refresh
            && self.mounted
            && causes.iter().all(|cause| {
                matches!(*cause, "views_frame" | "reveals" | "after_layout" | "al_motion")
            });
        if sidebar_refreshed {
            crate::shell::rebuild_stats::record_event("sidebar_subtree");
            ctx.request_frame(id, FrameReason::ANIMATION);
        } else if refresh && !(transcript_only && self.refresh_transcript(ctx, id, FrameReason::ANIMATION)) {
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
            set_window_scale(self.ui.ui_mut(), *scale as f32);
        }
        match event {
            WindowEvent::Resized(..) => {
                let rebuild = !self.mounted
                    || ctx
                        .window_state(id)
                        .is_none_or(|window| self.views.resize_needs_rebuild(&self.shell, window));
                self.drain(ctx, id, rebuild);
            }
            WindowEvent::ScaleChanged(..) | WindowEvent::Focused(..) => self.drain(ctx, id, true),
            _ => {}
        }
    }
    fn on_input(&mut self, ctx: &mut AppCtx<'_>, id: AquaWindowId, input: &InputEvent) {
        let _tm = Tm::new("input");
        let dismissed = {
            let _tm = Tm::new("input.dismiss");
            self.shell
                .entry
                .workspace_mut()
                .and_then(|workspace| workspace.composer.as_mut())
                .is_some_and(
                    |owner| match owner.dismiss_popups_for_event(self.ui.ui_mut(), input) {
                        Ok(consumed) => consumed,
                        Err(error) => {
                            tracing::warn!(%error, "could not dismiss composer popup");
                            false
                        }
                    },
                )
        };
        if dismissed {
            self.drain(ctx, id, false);
            return;
        }
        let mut glided = false;
        let consumed = {
            let _tm = Tm::new("input.route");
            let pre = {
                let _tm = Tm::new("input.pre");
                self.views.pre_input(self.ui.ui_mut(), input)
            };
            match pre {
                Some(consumed) => {
                    glided = matches!(input, InputEvent::Scroll { .. });
                    consumed
                }
                None => {
                    let _tm = Tm::new("input.ui");
                    self.ui.on_input(ctx, id, input);
                    self.ui.ui().last_input_consumed()
                }
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
                .input(&mut self.shell, self.ui.ui_mut(), ctx, id, input, consumed)
        };
        if glided && !changed {
            return;
        }
        let _tm_drain = Tm::new("input.drain");
        self.drain(ctx, id, changed);
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
        self.ui.on_a11y_action(ctx, id, request);
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
    fn chrome_regions(&self, window: &ObservedWindowState) -> Option<DragRegions> {
        self.shell
            .titlebar
            .regions()
            .or_else(|| self.ui.chrome_regions(window))
    }
    fn externals(&mut self, frame: &mut ExternalFrame<'_>) {
        let _tm = Tm::new("ext");
        self.views.advance_glides(&mut self.shell, self.ui.ui_mut());
        let supported = Some(false);
        if self.shell.meshes_supported.replace(supported) != supported {
            self.shell.meshes_changed.set(true);
            let _queued = self.shell.core.wake_mailbox().sender().send(());
        }
        if self
            .shell
            .live
            .remaining(std::time::Instant::now())
            .is_some_and(|wait| wait.is_zero())
        {
            let _queued = self.shell.core.wake_mailbox().sender().send(());
        }
        crate::conversation::open_timing::externals_begin();
        let started = std::time::Instant::now();
        self.ui.externals(frame);
        crate::conversation::open_timing::scene(started.elapsed());
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

fn keyed_node(ui: &UiApp, key: &str) -> Option<NodeId> {
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
    ui.tree().sole_node_with_key(key)
}

fn close_popup(ui: &mut UiApp, slot: &mut Option<aqua_ui::overlay::OverlayId>) {
    if let Some(id) = slot.take() {
        ui.close_popup(id);
    }
}

fn sync_popup(
    ui: &mut UiApp,
    slot: &mut Option<aqua_ui::overlay::OverlayId>,
    key: &str,
    content: Option<(Element, aqua_ui::popup::PopupOptions)>,
) {
    let Some((element, options)) = content else {
        close_popup(ui, slot);
        return;
    };
    if let Some(id) = *slot {
        if ui.host().is_open(id) {
            if let Err(error) = ui.refresh_popup(id, element) {
                tracing::warn!(%error, "could not refresh settings popup");
            }
            return;
        }
        *slot = None;
    }
    let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
    let mut nodes = ui.tree().nodes_with_key(hash).into_iter();
    let Some(owner) = nodes.next() else {
        return;
    };
    if nodes.next().is_some() {
        return;
    }
    match ui.open_popup_measured(owner, element, options) {
        Ok(id) => *slot = Some(id),
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

fn failure_card(message: &str, theme: crate::ui::theme::ArtisanTheme) -> Element {
    use crate::ui::theme::{RadiusStep, RadiusTokens};
    use aqua_text::{FontFamily, FontWeight, TextStyle};
    use aqua_ui::element::{Box as UiBox, Text};
    use aqua_ui::style::{Length, Style};
    let style = |size: f32, line: f32, weight: FontWeight| TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: size,
        line_height_px: line,
        weight,
        ..TextStyle::default()
    };
    let title = Text::new("Something went wrong drawing this view")
        .text_style(style(theme.typography.control_text, 20.0, FontWeight(600)))
        .gpui_source_color(theme.colors.text.to_reference_text_color())
        .build();
    let detail = Text::new(message.to_owned())
        .text_style(style(theme.typography.label_text, 19.0, FontWeight::NORMAL))
        .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
        .build();
    let card = UiBox::new()
        .layout("column")
        .gap(8.0)
        .padding(20.0)
        .width(480.0)
        .radius(RadiusTokens::value(RadiusStep::Xl))
        .style(Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
            theme.colors.banner_error.bg.to_reference_paint(),
        )))
        .child(title)
        .child(detail)
        .build();
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .layout("column")
        .style(
            Style::new()
                .align(aqua_ui::AlignItems::CENTER)
                .justify(aqua_ui::JustifyContent::CENTER)
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme.colors.background.to_reference_paint(),
                )),
        )
        .child(card)
        .build()
}

pub(crate) struct Tm(
    &'static str,
    std::time::Instant,
    aqua::profile::alloc::HeapCounts,
    crate::shell::rebuild_stats::Span,
);
impl Tm {
    pub(crate) fn threshold_ms() -> f64 {
        static VALUE: std::sync::OnceLock<f64> = std::sync::OnceLock::new();
        *VALUE.get_or_init(|| {
            std::env::var("AQUA_TM_MS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(3.0)
        })
    }

    pub(crate) fn new(name: &'static str) -> Self {
        crate::shell::rebuild_stats::record_event(name);
        Self(
            name,
            std::time::Instant::now(),
            aqua::profile::alloc::snapshot(),
            crate::shell::rebuild_stats::span(name),
        )
    }
}
impl Drop for Tm {
    fn drop(&mut self) {
        let ms = self.1.elapsed().as_secs_f64() * 1000.0;
        if aqua::profile::enabled() {
            let heap = aqua::profile::alloc::snapshot().since(self.2);
            if ms > 0.25 || heap.allocs > 8 {
                let name = self.0;
                aqua::profile::note(|| format!("{name}={ms:.2}ms/a{}", heap.allocs));
            }
        }
        if ms > Self::threshold_ms() {
            tracing::info!(target: "tm", what = self.0, ms);
        }
    }
}

fn mark(causes: &mut Vec<&'static str>, name: &'static str, changed: bool) -> bool {
    if changed {
        causes.push(name);
    }
    changed
}

#[cfg(all(test, feature = "qualification"))]
#[path = "../../tests/qualification/native.rs"]
mod native_tests;
