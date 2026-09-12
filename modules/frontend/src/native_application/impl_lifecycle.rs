//! Lifecycle, entity accessors, and route navigation for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    /// Creates the application root without doing process or network work.
    ///
    /// # Panics
    ///
    /// Panics if the bundled offline model catalog fails its built-in
    /// validation; the catalog is compile-time data, so a failure is a build
    /// defect rather than a runtime condition.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI composition keeps the arrangement in visual order; extraction would split the shared reactive state"
    )]
    pub(super) fn new(
        service: Option<Arc<NativeTransportService>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        #[cfg(test)]
        let add_project_focus_handle = cx.focus_handle().tab_index(0).tab_stop(true);
        let message_retry_focus_handle = cx.focus_handle().tab_index(2).tab_stop(false);
        focus_handle.focus(window, cx);
        let state = if service.is_some() {
            NativeViewState::Loading
        } else {
            NativeViewState::Failure(ServiceFailure {
                stage: ServiceFailureStage::EventBridge,
                category: ServiceFailureCategory::ChannelClosed,
            })
        };
        let composer = cx.new(NativeComposer::new);
        let composer_controls =
            cx.new(|cx| NativeComposerControls::new(NativeComposerControlsSnapshot::idle(), cx));
        let catalog =
            NativeModelCatalog::offline().expect("the bundled model catalog is validated");
        let model_selector =
            cx.new(|cx| NativeModelSelector::new(catalog, None, ThemeMode::Dark, cx));
        let composer_controls_subscription = cx.subscribe(&composer_controls, |application, _, event, cx| {
            if application.handle_queue_control(event, cx) { return; }
            match event {
                NativeComposerControlsEvent::SendRequested => application.begin_message_submission(cx),
                NativeComposerControlsEvent::JumpToLatest => {
                    if let Some(host) = application.conversation_host.clone() {
                        let result = host.update(cx, |host, cx| host.dispatch(
                            ConversationStateEvent::Viewport(crate::conversation_view_machine::ViewportEvent::JumpToBottomRequested), cx));
                        if result.is_err() { application.set_failure(invalid_service_failure(), cx); }
                        else { application.pump_host_boundary(&host, cx); }
                    }
                }
                NativeComposerControlsEvent::StopRequested { run_id } => application.stop_composer_run(run_id, cx),
                NativeComposerControlsEvent::StartNewThreadWithPrompt { run_id } => {
                    let snapshot = application.composer_controls.read(cx).snapshot();
                    if snapshot.new_thread_ready && snapshot.run_id.as_deref() == Some(run_id.as_str()) {
                        application.begin_new_task(cx);
                    }
                }
                NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
                    command_id,
                    generation,
                } => {
                    application.begin_failed_prompt_recovery(command_id, *generation, cx);
                }
                NativeComposerControlsEvent::DismissFailure { failure_id } => {
                    if application.message_failure.is_some_and(|failure| failure.id == *failure_id) {
                        application.message_failure = None;
                        application.message_failure_note = None;
                        application.clear_message_retry();
                        application.sync_composer_controls(cx);
                    }
                }
                NativeComposerControlsEvent::RetryFailure { failure_id }
                    if application.message_failure.is_some_and(|failure| failure.id == *failure_id) => {
                        application.activate_message_retry(cx);
                        application.sync_composer_controls(cx);
                    }
                _ => {}
            }
        });
        let composer_model_subscription =
            cx.subscribe(&model_selector, |application, _, event, cx| {
                application.handle_composer_model_event(event, cx);
            });
        composer.update(cx, |composer, cx| {
            composer.set_components(&composer_controls, &model_selector, cx);
        });
        let message_images = cx.new(NativeMessageImages::new);
        let message_images_subscription =
            cx.subscribe(&message_images, |application, images, event, cx| {
                let NativeMessageImagesEvent::RequestImage(reference) = event;
                if let Err(error) = application
                    .submit_command(NativeTransportCommand::ReadMessageImage(reference.clone()))
                {
                    images.update(cx, |images, cx| {
                        images.fail_image(reference.clone(), command_failure(error), cx);
                    });
                }
            });
        composer.update(cx, |composer, cx| {
            composer.set_attachment_delivery_enabled(true, cx);
        });
        let composer_subscription =
            cx.subscribe(&composer, |application, _composer, event, cx| match event {
                NativeComposerEvent::SendRequested => application.begin_message_submission(cx),
                NativeComposerEvent::ConfigureModel => application.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::Engines,
                        engine: None,
                    },
                    cx,
                ),
            });
        let composer_observation = cx.observe(&composer, |application, composer, cx| {
            application.observe_composer_change(&composer, cx);
        });
        let command_menu = cx.new(|menu_cx| {
            NativeCommandMenu::new(vec![CommandMenuGroup::actions()], ThemeMode::Dark, menu_cx)
        });
        let command_menu_observation = cx.observe(&command_menu, |application, menu, cx| {
            application.route_command_action(&menu, cx);
        });
        let profile_hostname = std::env::var("COMPUTERNAME")
            .ok()
            .filter(|name| !name.is_empty());
        let profile_name = std::env::var("USERNAME")
            .ok()
            .filter(|name| !name.is_empty())
            .or_else(|| profile_hostname.clone());
        let mut application = Self {
            theme: ArtisanTheme::for_mode(ThemeMode::Dark),
            desktop_theme: DesktopTheme::neutral_dark(),
            focus_handle,
            #[cfg(test)]
            add_project_focus_handle,
            message_retry_focus_handle,
            service,
            composer,
            run_controls: composer_run_controls::RunControlsState::default(),
            composer_queue: composer_queue_application::QueueApplicationState::new(cx),
            composer_controls,
            model_selector,
            composer_model_choice: None,
            composer_model_run_error: None,
            pending_failed_recovery: None,
            catalog_controller: NativeCatalogController::new(),
            _composer_controls_subscription: composer_controls_subscription,
            _composer_model_subscription: composer_model_subscription,
            _composer_subscription: composer_subscription,
            message_images,
            _message_images_subscription: message_images_subscription,
            _composer_observation: composer_observation,
            profile_menu: DropdownMenuState::new([
                DropdownMenuEntry::item(DropdownMenuItem::new("settings", "Settings")),
                DropdownMenuEntry::item(DropdownMenuItem::new("usage", "Usage")),
            ]),
            profile_focus: cx.focus_handle(),
            profile_origin: Rc::new(Cell::new(Bounds::default())),
            profile_picture: None,
            profile_name,
            profile_hostname,
            profile_usage: NativeProfileUsageState::default(),
            profile_usage_generation: ProfileUsageGeneration::first(),
            profile_usage_next_seq: 0,
            profile_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            profile_hover_surface_bounds: Rc::new(RefCell::new(None)),
            profile_hover_keyboard: Cell::new(false),
            profile_usage_scroll: ScrollHandle::new(),
            profile_usage_scroll_state: PickerScrollState::default(),
            profile_usage_scroll_frame_scheduled: false,
            profile_meter_hover: Rc::new(RefCell::new(None)),
            profile_tip_surface_bounds: Rc::new(RefCell::new(None)),
            profile_tip_anchor: Rc::new(RefCell::new(None)),
            profile_refresh_focus: Rc::new(RefCell::new(Vec::new())),
            profile_refresh_swap: Rc::new(RefCell::new(HashMap::new())),
            profile_swap_frame_scheduled: Rc::new(Cell::new(false)),
            profile_menu_motion: Rc::new(RefCell::new(PickerMenuMotion::default())),
            profile_menu_motion_task: None,
            profile_tip_tween: Rc::new(RefCell::new(ProfileTipTween::default())),
            command_menu,
            _command_menu_observation: command_menu_observation,
            sidebar_collapsed: false,
            sidebar_navigation_focus: cx.focus_handle(),
            sidebar_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            sidebar_hover_surface_bounds: Rc::new(RefCell::new(None)),
            message_flight: None,
            message_retry: None,
            message_receipt: None,
            message_failure: None,
            message_failure_note: None,
            picker: None,
            picker_subscription: None,
            home_picker: None,
            home_picker_subscription: None,
            project_options: Vec::new(),
            selected_project: None,
            titlebar_repository: None,
            titlebar_repository_project: None,
            thread_listing: None,
            sidebar_threads: impl_sidebar_threads::SidebarThreadsState::default(),
            selected_thread: None,
            pending_thread: None,
            thread_picker: None,
            thread_picker_subscription: None,
            thread_switch_flight: None,
            next_thread_switch_generation: 0,
            active_subscription_request_id: None,
            ordinary_unsubscribe_thread: None,
            standalone_snapshot_thread: None,
            retained_switch_request_ids: Vec::with_capacity(MAX_RETAINED_SWITCH_REQUEST_IDS),
            retained_switch_patch_ids: Vec::with_capacity(MAX_RETAINED_SWITCH_PATCH_IDS),
            retained_switch_snapshot_threads: Vec::with_capacity(MAX_RETAINED_SWITCH_REQUEST_IDS),
            retained_switch_listings: Vec::with_capacity(MAX_RETAINED_SWITCH_LISTINGS),
            pending_snapshot: None,
            conversation_host: None,
            conversation_host_subscription: None,
            conversation_effects: Vec::with_capacity(CONVERSATION_HOST_MAX_EFFECTS),
            engine_observations: None,
            last_picker_action: None,
            state,
            route_history: RouteHistory::new(),
            onboarding_screen: None,
            thread_screen: None,
            thread_screen_key: None,
            editor_screen: None,
            editor_screen_key: None,
            settings_screen: None,
            settings_screen_key: None,
            settings_screen_subscription: None,
            intake_stage: None,
            intake_failure_operation: None,
            intake_retry_available: false,
            intake_restore_state: None,
            service_stopped: false,
            shutdown_prepared: false,
            #[cfg(test)]
            test_command_sink: None,
            poll_task: None,
            engine_settings: EngineSettingsController::new(),
        };
        let return_focus = application.focus_handle.clone();
        application.command_menu.update(cx, |menu, _| {
            menu.set_return_focus(return_focus);
        });
        application.sync_command_menu_groups(cx);
        application.sync_composer_availability(cx);
        #[cfg(not(test))]
        cx.spawn(async move |view, cx| {
            let picture = cx
                .background_executor()
                .spawn(async { crate::shell::local_account_picture() })
                .await;
            let _ = view.update(cx, |app, cx| {
                app.profile_picture = picture;
                cx.notify();
            });
        })
        .detach();
        // The Forge publishes its scope-free catalog snapshot shortly after it
        // starts (discovery warms in the background); its first discovery is
        // cold, so an older snapshot may already exist on disk. Watch through
        // that window and re-apply whenever the revision changes.
        #[cfg(not(test))]
        cx.spawn(async move |view, cx| {
            let mut applied: Option<String> = None;
            for _ in 0..90 {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(1))
                    .await;
                let revision = cx
                    .background_executor()
                    .spawn(async {
                        scope_free_catalog_snapshot().map(|catalog| catalog.catalog_revision)
                    })
                    .await;
                let Some(revision) = revision else {
                    continue;
                };
                if applied.as_deref() == Some(revision.as_str()) {
                    continue;
                }
                applied = Some(revision);
                let _ = view.update(cx, |app, cx| {
                    if app.selected_thread.is_none() {
                        app.reset_model_selector_offline(cx);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
        application
    }

    /// Begins the application-thread poller for service events.
    pub(super) fn start_polling(&mut self, cx: &mut Context<Self>) {
        if self.service.is_none() || self.poll_task.is_some() {
            return;
        }
        let task = cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(POLL_INTERVAL).await;
                let Some(keep_polling) = view.update(cx, NativeApplication::poll_service).ok()
                else {
                    break;
                };
                if !keep_polling {
                    break;
                }
            }
        });
        self.poll_task = Some(task);
    }

    /// Returns the current navigation route.
    #[must_use]
    pub const fn route(&self) -> &NativeRoute {
        self.route_history.current()
    }

    /// Navigates to `route`, retaining history, and rerenders.
    ///
    /// Entering Settings also requests fresh provider-account reads, so the
    /// engine pages observe true readiness instead of a stale row; the
    /// profile popover keeps its own open-time refresh.
    pub(super) fn navigate(&mut self, route: NativeRoute, cx: &mut Context<Self>) {
        if matches!(route, NativeRoute::Settings { .. }) {
            self.ensure_profile_usage(false, None, cx);
        }
        self.route_history.navigate(route);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    #[cfg(test)]
    /// Returns to the previous route when history exists, then rerenders.
    pub(super) fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        let moved = self.route_history.go_back();
        if moved {
            self.sync_composer_availability(cx);
            cx.notify();
        }
        moved
    }
}
