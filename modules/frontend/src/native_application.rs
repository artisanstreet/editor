//! Shipping GPUI application assembly for the native Artisan workflow.
//!
//! This module owns only application-thread composition. Installation,
//! transport, process custody, and protocol work live in
//! `native_transport_service`; the conversation host remains the sole owner of
//! controller and surface policy.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    cell::{Cell, RefCell},
    process::ExitCode,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

#[cfg(test)]
use std::collections::VecDeque;

use artisan_assets::AssetId;
use artisan_domain::{
    CatalogRevision, ConversationSnapshot, EngineProfileId, ModelFavoriteId, PatchBatch, ProjectId,
    ProjectListing, QueueMessagePayload, RequestId, SetModelFavorite, ThreadId, ThreadListing,
};
use artisan_protocol::{ConversationSubscriptionStarted, QueueMessageReceipt};
use artisan_ui::button::{
    AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility,
};
use artisan_ui::card::{CardStyle, compact_card, compact_card_content};
use artisan_ui::dropdown_menu::{DropdownMenuEntry, DropdownMenuItem, DropdownMenuState};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::tabs::{TabSpec, Tabs};
use artisan_ui::theme::{ArtisanTheme, DesktopTheme, ThemeMode};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, ClipboardItem, Context, Div, Entity,
    FocusHandle, FontWeight, KeyBinding, Render, SharedString, Stateful,
    StatefulInteractiveElement, StyledImage as _, Subscription, Task, TitlebarOptions, Window,
    WindowBounds, WindowOptions, actions, div,
    prelude::{InteractiveElement as _, IntoElement, ParentElement as _, Styled as _},
    px, size,
};

use crate::composer::{DraftDisposition, SubmissionBlocked, SubmissionToken};
use crate::desktop_shell::{
    DESKTOP_COMPOSER_SELECTOR, DESKTOP_HOME_SELECTOR, desktop_muted, desktop_nav_glyph,
    desktop_shell,
};
use crate::editor_route_screen::{EditorScreen, EditorScreenIdentity, EditorSurfaceState};
use crate::native_command_menu::{
    CommandMenuAction, CommandMenuEntry, CommandMenuGroup, NativeCommandMenu,
};
use crate::native_composer::{NativeComposer, NativeComposerEvent};
use crate::native_composer_controls::{
    NativeComposerControls, NativeComposerControlsEvent, NativeComposerControlsSnapshot,
};
use crate::native_message_images::{NativeMessageImages, NativeMessageImagesEvent};
use crate::native_model_catalog::NativeModelCatalog;
use crate::native_model_selector::{NativeModelSelector, NativeModelSelectorStatus};
use crate::native_route::{NativeRoute, RouteHistory, SettingsRoute};
use crate::native_settings::SettingsScreen;
use crate::native_transport::{
    CatalogLoadGeneration, CatalogScopeError, NativeCatalogController, NativeCatalogPhase,
    NativeCatalogScope,
};
use crate::native_transport_service::{
    CommandSendError, EventReceiveError, NativeProjectIntakeOperation, NativeProjectIntakeStage,
    NativeTransportCommand, NativeTransportEvent, NativeTransportService, ServiceFailure,
    ServiceFailureCategory, ServiceFailureStage, ServiceStopStatus, SettingsLoadGeneration,
};
use crate::onboarding_harness_presentation::{
    HarnessCatalog, HarnessSetupAction, HarnessSetupState,
};
use crate::onboarding_screen::{OnboardingHarnessEntry, OnboardingScreen};
use crate::thread_screen::{ThreadScreen, ThreadScreenGate};
use crate::workspace_tab_state::EditorViewState;
use crate::{
    conversation_delivery_machine::{ConversationDeliveryEffect, ConversationDeliveryEvent},
    conversation_host::{CONVERSATION_HOST_MAX_EFFECTS, ConversationHost, ConversationHostEffect},
    conversation_state_machine::{ConversationStateEffect, ConversationStateEvent},
    conversation_view_machine::ViewportState,
    engine_settings::{
        EngineSettingsController, EngineSettingsFailureOperation, EngineSettingsStatus,
        RegistryView, manual_configuration_template,
    },
    native_thread_picker::{NativeThreadPicker, ThreadPickerAction},
    project_picker::{ProjectOption, ProjectPickerAction, ProjectPickerView},
    thread_title_policy::{ThreadTitleInput, ThreadTitleMode, thread_display_title},
};

actions!(
    native_application,
    [Quit, NextTabStop, PreviousTabStop, OpenCommandMenu]
);

/// The one shipping application title.
pub(crate) const WINDOW_TITLE: &str = "Artisan Editor";

/// Stable selector for the real application root.
pub(crate) const NATIVE_ROOT_SELECTOR: &str = "artisan-native-application";

/// Stable selector for the state panel.
pub(crate) const NATIVE_STATUS_SELECTOR: &str = "artisan-native-status";

/// Stable selector for the engine-settings section.
pub(crate) const NATIVE_ENGINE_SETTINGS_SELECTOR: &str = "artisan-native-engine-settings";

/// Stable selector for the rail's add-project action.
pub(crate) const NATIVE_RAIL_ADD_PROJECT_SELECTOR: &str = "artisan-native-rail-add-project";

/// Accessible name retained by the rail's icon-only add-project action.
pub(crate) const NATIVE_RAIL_ADD_PROJECT_LABEL: &str = "Add project";

/// Stable selector for the explicit first-message retry action.
pub(crate) const NATIVE_MESSAGE_RETRY_SELECTOR: &str = "artisan-native-message-retry";

/// Visible and accessible name retained by the first-message retry action.
const NATIVE_MESSAGE_RETRY_LABEL: &str = "Retry send";

const NATIVE_KEY_CONTEXT: &str = "artisan-native-application";
const SURFACE_WIDTH: f32 = 1_024.0;
const SURFACE_HEIGHT: f32 = 720.0;
const POLL_INTERVAL: Duration = Duration::from_millis(16);

#[cfg(test)]
#[derive(Clone)]
struct NativeTestCommandSink {
    commands: Rc<RefCell<Vec<NativeTransportCommand>>>,
    outcomes: Rc<RefCell<VecDeque<Result<(), CommandSendError>>>>,
}

/// Application-facing state; every branch is honest about the native
/// milestone and contains no fixture catalog.
#[derive(Clone)]
enum NativeViewState {
    Loading,
    EmptyProjects,
    LoadingThreads,
    EmptyThreads,
    Ready,
    Failure(ServiceFailure),
}

/// Application-owned identity for one admitted message queue.
///
/// The body is intentionally retained here until the application observes a
/// correlated terminal result. This type owns message text and therefore
/// implements neither `Debug` nor `Display`.
struct NativeMessageFlight {
    thread_id: ThreadId,
    request_id: RequestId,
    payload: QueueMessagePayload,
    token: SubmissionToken,
}

/// Application-owned identity for one explicitly retryable message
/// queue. This type owns message text and therefore implements neither
/// `Debug` nor `Display`.
struct NativeMessageRetry {
    thread_id: ThreadId,
    request_id: RequestId,
    payload: QueueMessagePayload,
    draft_matches: bool,
}

#[derive(Clone, Copy)]
struct NativeMessageFailure {
    failure: ServiceFailure,
    id: u64,
}
impl NativeMessageFailure {
    fn new(failure: ServiceFailure) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .expect("failure identity exhausted");
        Self { failure, id }
    }
}

/// One finite, generation-fenced transition between mounted conversations.
///
/// Request IDs are deliberately optional until the service reports the
/// corresponding admission receipt. The service owns request-ID minting;
/// this flight only records the receipt that advanced its current phase.
struct ThreadSwitchFlight {
    source_thread: ThreadId,
    target_thread: Option<ThreadId>,
    generation: u64,
    phase: ThreadSwitchPhase,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ThreadSwitchPhase {
    /// The source unsubscribe has not yet entered the service queue.
    UnsubscribeAdmission {
        retry_pending: bool,
        retry_used: bool,
    },
    /// The service accepted the source unsubscribe and must acknowledge it.
    AwaitingUnsubscribeStop { request_id: Option<RequestId> },
    /// The stop receipt was accepted; the old host is being retired locally.
    HostRetirement { request_id: RequestId },
    /// The target is ready to mount and its fresh subscription is admitted.
    SubscribeAdmission {
        retry_pending: bool,
        retry_used: bool,
    },
    /// The fresh target subscription was admitted and must start.
    AwaitingSubscriptionStart { request_id: Option<RequestId> },
}

const MAX_RETAINED_SWITCH_REQUEST_IDS: usize = 8;
const MAX_RETAINED_SWITCH_PATCH_IDS: usize = 256;
const MAX_RETAINED_SWITCH_LISTINGS: usize = 8;

/// The real native window root and its application-thread entities.
pub struct NativeApplication {
    theme: ArtisanTheme,
    desktop_theme: DesktopTheme,
    focus_handle: FocusHandle,
    add_project_focus_handle: FocusHandle,
    message_retry_focus_handle: FocusHandle,
    service: Option<Arc<NativeTransportService>>,
    composer: Entity<NativeComposer>,
    run_controls: composer_run_controls::RunControlsState,
    composer_queue: composer_queue_application::QueueApplicationState,
    composer_controls: Entity<NativeComposerControls>,
    model_selector: Entity<NativeModelSelector>,
    catalog_controller: NativeCatalogController,
    _composer_controls_subscription: Subscription,
    _composer_model_subscription: Subscription,
    message_images: Entity<NativeMessageImages>,
    _message_images_subscription: Subscription,
    _composer_subscription: Subscription,
    _composer_observation: Subscription,
    profile_menu: DropdownMenuState,
    profile_focus: FocusHandle,
    profile_origin: Rc<Cell<Bounds<gpui::Pixels>>>,
    profile_picture: Option<std::path::PathBuf>,
    profile_name: Option<String>,
    profile_hostname: Option<String>,
    command_menu: Entity<NativeCommandMenu>,
    _command_menu_observation: Subscription,
    sidebar_collapsed: bool,
    sidebar_editor: bool,
    sidebar_tabs_focus: FocusHandle,
    sidebar_navigation_focus: FocusHandle,
    message_flight: Option<NativeMessageFlight>,
    message_retry: Option<NativeMessageRetry>,
    message_receipt: Option<QueueMessageReceipt>,
    message_failure: Option<NativeMessageFailure>,
    picker: Option<Entity<ProjectPickerView>>,
    picker_subscription: Option<Subscription>,
    project_options: Vec<ProjectOption>,
    selected_project: Option<ProjectId>,
    /// The latest authoritative thread listing for `selected_project`.
    thread_listing: Option<ThreadListing>,
    selected_thread: Option<ThreadId>,
    pending_thread: Option<ThreadId>,
    thread_picker: Option<Entity<NativeThreadPicker>>,
    thread_picker_subscription: Option<Subscription>,
    thread_switch_flight: Option<ThreadSwitchFlight>,
    next_thread_switch_generation: u64,
    active_subscription_request_id: Option<RequestId>,
    ordinary_unsubscribe_thread: Option<ThreadId>,
    /// Once a subscription has started, its fresh/resumed response is the
    /// baseline for that host. A later standalone snapshot has no generation
    /// receipt and therefore cannot replace it.
    standalone_snapshot_thread: Option<ThreadId>,
    retained_switch_request_ids: Vec<RequestId>,
    retained_switch_patch_ids: Vec<artisan_domain::PatchId>,
    retained_switch_snapshot_threads: Vec<ThreadId>,
    retained_switch_listings: Vec<ThreadListing>,
    pending_snapshot: Option<ConversationSnapshot>,
    conversation_host: Option<Entity<ConversationHost>>,
    conversation_host_subscription: Option<Subscription>,
    conversation_effects: Vec<ConversationHostEffect>,
    last_picker_action: Option<ProjectPickerAction>,
    state: NativeViewState,
    route_history: RouteHistory,
    onboarding_screen: Option<Entity<OnboardingScreen>>,
    thread_screen: Option<Entity<ThreadScreen>>,
    thread_screen_key: Option<(ThreadId, bool)>,
    editor_screen: Option<Entity<EditorScreen>>,
    editor_screen_key: Option<(ProjectId, ThreadId, Option<String>)>,
    settings_screen: Option<Entity<SettingsScreen>>,
    settings_screen_key: Option<(SettingsRoute, Option<String>)>,
    intake_stage: Option<NativeProjectIntakeStage>,
    intake_failure_operation: Option<NativeProjectIntakeOperation>,
    intake_retry_available: bool,
    intake_restore_state: Option<NativeViewState>,
    service_stopped: bool,
    shutdown_prepared: bool,
    #[cfg(test)]
    test_command_sink: Option<NativeTestCommandSink>,
    poll_task: Option<Task<()>>,
    engine_settings: EngineSettingsController,
}

impl NativeApplication {
    /// Creates the application root without doing process or network work.
    #[must_use]
    pub fn new(
        service: Option<Arc<NativeTransportService>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
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
                NativeComposerControlsEvent::DismissFailure { failure_id } => {
                    if application.message_failure.is_some_and(|failure| failure.id == *failure_id) {
                        application.message_failure = None;
                        application.clear_message_retry();
                        application.sync_composer_controls(cx);
                    }
                }
                NativeComposerControlsEvent::RetryFailure { failure_id } => {
                    if application.message_failure.is_some_and(|failure| failure.id == *failure_id) {
                        application.activate_message_retry(cx);
                        application.sync_composer_controls(cx);
                    }
                }
                _ => {}
            }
        });
        let composer_model_subscription =
            cx.subscribe(&model_selector, |application, _, event, cx| {
                application.handle_composer_model_event(event, cx);
            });
        composer.update(cx, |composer, cx| {
            composer.set_components(composer_controls.clone(), model_selector.clone(), cx)
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
                .titlebar_mode()
        });
        let command_menu_observation = cx.observe(&command_menu, |application, menu, cx| {
            application.route_command_action(&menu, cx);
        });
        let mut application = Self {
            theme: ArtisanTheme::for_mode(ThemeMode::Dark),
            desktop_theme: DesktopTheme::neutral_dark(),
            focus_handle,
            add_project_focus_handle,
            message_retry_focus_handle,
            service,
            composer,
            run_controls: composer_run_controls::RunControlsState::default(),
            composer_queue: composer_queue_application::QueueApplicationState::new(cx),
            composer_controls,
            model_selector,
            catalog_controller: NativeCatalogController::new(),
            _composer_controls_subscription: composer_controls_subscription,
            _composer_model_subscription: composer_model_subscription,
            _composer_subscription: composer_subscription,
            message_images,
            _message_images_subscription: message_images_subscription,
            _composer_observation: composer_observation,
            profile_menu: DropdownMenuState::new([
                DropdownMenuEntry::item(DropdownMenuItem::new("settings", "Settings")),
                DropdownMenuEntry::item(DropdownMenuItem::new("add-project", "Add project")),
            ]),
            profile_focus: cx.focus_handle(),
            profile_origin: Rc::new(Cell::new(Bounds::default())),
            profile_picture: None,
            profile_name: std::env::var("USERNAME")
                .ok()
                .filter(|name| !name.is_empty()),
            profile_hostname: std::env::var("COMPUTERNAME")
                .ok()
                .filter(|name| !name.is_empty()),
            command_menu,
            _command_menu_observation: command_menu_observation,
            sidebar_collapsed: false,
            sidebar_editor: false,
            sidebar_tabs_focus: cx.focus_handle(),
            sidebar_navigation_focus: cx.focus_handle(),
            message_flight: None,
            message_retry: None,
            message_receipt: None,
            message_failure: None,
            picker: None,
            picker_subscription: None,
            project_options: Vec::new(),
            selected_project: None,
            thread_listing: None,
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
        application
    }

    /// Begins the application-thread poller for service events.
    fn start_polling(&mut self, cx: &mut Context<Self>) {
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

    /// Returns the picker entity once real project rows have arrived.
    #[must_use]
    pub fn picker(&self) -> Option<&Entity<ProjectPickerView>> {
        self.picker.as_ref()
    }

    /// Returns the native thread picker after the authoritative thread
    /// listing has arrived.
    #[must_use]
    pub fn thread_picker(&self) -> Option<&Entity<NativeThreadPicker>> {
        self.thread_picker.as_ref()
    }

    /// Returns the current authoritative thread listing.
    #[must_use]
    pub fn thread_listing(&self) -> Option<&ThreadListing> {
        self.thread_listing.as_ref()
    }

    /// Returns the thread identity currently mounted, if any.
    #[must_use]
    pub fn selected_thread(&self) -> Option<&ThreadId> {
        self.selected_thread.as_ref()
    }

    /// Returns the current navigation route.
    #[must_use]
    pub const fn route(&self) -> &NativeRoute {
        self.route_history.current()
    }

    /// Navigates to `route`, retaining history, and rerenders.
    pub fn navigate(&mut self, route: NativeRoute, cx: &mut Context<Self>) {
        match &route {
            NativeRoute::Editor { .. } => self.sidebar_editor = true,
            NativeRoute::Thread { .. } | NativeRoute::NewThread { .. } => {
                self.sidebar_editor = false
            }
            _ => {}
        }
        self.route_history.navigate(route);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    /// Returns to the previous route when history exists, then rerenders.
    pub fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        let moved = self.route_history.go_back();
        if moved {
            match self.route() {
                NativeRoute::Editor { .. } => self.sidebar_editor = true,
                NativeRoute::Thread { .. } | NativeRoute::NewThread { .. } => {
                    self.sidebar_editor = false
                }
                _ => {}
            }
            self.sync_composer_availability(cx);
            cx.notify();
        }
        moved
    }

    /// Renders the concise new-task home surface. All actions and catalog
    /// rows live in the shell/sidebar; this route only explains the current
    /// real state and leaves the editable composer to the shell footer.
    fn new_thread_surface_section(&self) -> Div {
        let (heading, detail) = if self.project_options.is_empty() {
            (
                "Start a task",
                "Add a project to give Artisan a workspace to work in.".to_owned(),
            )
        } else if matches!(&self.state, NativeViewState::Failure(_)) {
            (
                "Start a task",
                "Forge is offline. Existing project and task data will remain visible when it reconnects."
                    .to_owned(),
            )
        } else if let Some(project) = self.selected_project_name() {
            (
                "Start a task",
                format!("Choose a task in {project}, or describe what you need below."),
            )
        } else {
            (
                "Start a task",
                "Choose a project from the sidebar, then describe what you need below.".to_owned(),
            )
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(24.0))
            .pb(px(24.0))
            .debug_selector(|| DESKTOP_HOME_SELECTOR.to_string())
            .child(
                div()
                    .text_size(px(20.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(self.desktop_theme.foreground)
                    .child(heading),
            )
            .child(
                div()
                    .mt(px(8.0))
                    .max_w(px(440.0))
                    .text_size(px(15.0))
                    .text_color(self.desktop_theme.secondary)
                    .child(detail),
            )
    }

    fn selected_project_name(&self) -> Option<String> {
        self.selected_project.as_ref().and_then(|selected| {
            self.project_options
                .iter()
                .find(|option| &option.id == selected)
                .map(|option| option.name.to_string())
        })
    }

    fn sync_command_menu_groups(&mut self, cx: &mut Context<Self>) {
        let mut groups = vec![CommandMenuGroup::actions()];
        if !self.project_options.is_empty() {
            groups.push(CommandMenuGroup::new(
                "projects",
                "Projects",
                self.project_options
                    .iter()
                    .map(|project| {
                        CommandMenuEntry::project(project.id.as_str(), project.name.to_string())
                    })
                    .collect(),
            ));
        }
        if let Some(listing) = self.thread_listing.as_ref() {
            let project_id = self.selected_project.as_ref();
            let entries = listing
                .threads()
                .iter()
                .filter(|thread| project_id == Some(&thread.project_id))
                .map(|thread| {
                    let title = thread_display_title(
                        ThreadTitleInput {
                            summary_title: None,
                            title: thread.title.as_str(),
                            title_locked: false,
                        },
                        ThreadTitleMode::default(),
                    );
                    CommandMenuEntry::thread(
                        thread.thread_id.as_str(),
                        title.to_owned(),
                        thread.title.as_str(),
                    )
                })
                .collect();
            let (group_id, heading) = self
                .selected_project
                .as_ref()
                .map(|project| {
                    (
                        format!("tasks-{}", project.as_str()),
                        self.selected_project_name()
                            .map_or_else(|| "Tasks".to_owned(), |name| format!("{name} tasks")),
                    )
                })
                .unwrap_or_else(|| (String::from("tasks"), String::from("Tasks")));
            groups.push(CommandMenuGroup::new(group_id, heading, entries));
        }
        let command_menu = self.command_menu.clone();
        command_menu.update(cx, |menu, menu_cx| {
            menu.replace_groups(groups, menu_cx);
        });
    }

    fn route_command_action(&mut self, menu: &Entity<NativeCommandMenu>, cx: &mut Context<Self>) {
        let Some(action) = menu.update(cx, |menu, _| menu.take_pending_action()) else {
            return;
        };
        match action {
            CommandMenuAction::NewThread => self.begin_new_task(cx),
            CommandMenuAction::OpenSettings => {
                self.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::Models,
                        engine: None,
                    },
                    cx,
                );
            }
            CommandMenuAction::OpenProject { project_id } => {
                let Some(project) = self
                    .project_options
                    .iter()
                    .find(|project| project.id.as_str() == project_id)
                    .map(|project| project.id.clone())
                else {
                    return;
                };
                self.select_project_from_sidebar(project, cx);
            }
            CommandMenuAction::OpenThread { thread_id } => {
                let Some(thread) = self
                    .thread_listing
                    .as_ref()
                    .and_then(|listing| {
                        listing
                            .threads()
                            .iter()
                            .find(|thread| thread.thread_id.as_str() == thread_id)
                    })
                    .map(|thread| thread.thread_id.clone())
                else {
                    return;
                };
                self.open_thread_from_sidebar(thread, cx);
            }
        }
    }

    fn activate_command_menu(
        &mut self,
        _: &OpenCommandMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = self.command_menu.clone();
        menu.update(cx, |menu, menu_cx| {
            menu.focus_or_open(window, menu_cx);
        });
    }

    fn dismiss_command_menu(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = self.command_menu.clone();
        if menu.read(cx).state().is_open() {
            menu.update(cx, |menu, menu_cx| menu.dismiss(window, menu_cx));
        }
    }

    fn activate_new_task(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.begin_new_task(cx);
    }

    fn begin_new_task(&mut self, cx: &mut Context<Self>) {
        if !self.add_project_action_is_admissible() {
            return;
        }
        if self.intake_retry_available || self.selected_project.is_none() {
            self.submit_intake_command(cx);
            if self.intake_stage.is_some() {
                self.navigate(
                    NativeRoute::NewThread {
                        project: self.selected_project.clone(),
                    },
                    cx,
                );
            }
            return;
        }
        let project = self
            .selected_project
            .clone()
            .expect("selected project checked above");
        match self.submit_command(NativeTransportCommand::CreateTask(project)) {
            Ok(()) => {
                self.handle_intake_progress(NativeProjectIntakeStage::CreatingThread, cx);
                self.state = NativeViewState::Loading;
                self.navigate(
                    NativeRoute::NewThread {
                        project: self.selected_project.clone(),
                    },
                    cx,
                );
            }
            Err(error) => self.handle_intake_failed(
                NativeProjectIntakeOperation::CreateThread,
                command_failure(error),
                false,
                cx,
            ),
        }
    }

    fn activate_settings(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.navigate(
            NativeRoute::Settings {
                section: SettingsRoute::Models,
                engine: None,
            },
            cx,
        );
    }

    fn toggle_sidebar(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        cx.notify();
    }

    fn select_project_from_sidebar(&mut self, project_id: ProjectId, cx: &mut Context<Self>) {
        if !self
            .project_options
            .iter()
            .any(|project| &project.id == &project_id)
            || !self.project_picker_action_is_admissible()
        {
            return;
        }
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.intake_stage = None;
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        self.intake_restore_state = None;
        self.pending_thread = None;
        self.pending_snapshot = None;
        if self.selected_project.as_ref() != Some(&project_id) {
            self.retire_host(cx);
            self.thread_listing = None;
            self.retained_switch_listings.clear();
            self.install_thread_picker(empty_thread_listing(), None, cx);
        }
        self.selected_project = Some(project_id.clone());
        self.navigate(
            NativeRoute::NewThread {
                project: Some(project_id.clone()),
            },
            cx,
        );
        self.set_thread_picker_disabled(true, cx);
        self.state = NativeViewState::Loading;
        self.sync_command_menu_groups(cx);
        self.sync_composer_availability(cx);
        match self.submit_command(NativeTransportCommand::SelectProject(project_id)) {
            Ok(()) => cx.notify(),
            Err(error) => self.set_failure(command_failure(error), cx),
        }
    }

    fn open_thread_from_sidebar(&mut self, thread_id: ThreadId, cx: &mut Context<Self>) {
        if !self.thread_is_listed(&thread_id) || !self.project_picker_action_is_admissible() {
            return;
        }
        let Some(project) = self.selected_project.clone() else {
            return;
        };
        if self.selected_thread.as_ref() == Some(&thread_id) {
            self.navigate(
                NativeRoute::Thread {
                    project,
                    thread: thread_id,
                },
                cx,
            );
            self.sync_composer_availability(cx);
            return;
        }
        if self.selected_thread.is_none() || self.conversation_host.is_none() {
            self.selected_thread = None;
            self.pending_thread = Some(thread_id);
            self.state = NativeViewState::Loading;
            self.try_mount_pending_thread(cx);
        } else {
            self.begin_thread_switch(thread_id, cx);
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn desktop_identity(&self, cx: &Context<Self>) -> Div {
        let mut identity = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .child(
                div()
                    .id("artisan-brand-home")
                    .cursor_pointer()
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.navigate(NativeRoute::NewThread { project: None }, cx);
                    }))
                    .debug_selector(|| "artisan-brand-home".to_owned())
                    .flex_shrink_0()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::EXTRA_BOLD)
                    .text_color(self.desktop_theme.foreground)
                    .child("Artisan Editor"),
            );
        if let Some(project) = self.selected_project_name() {
            identity = identity.child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .whitespace_nowrap()
                    .text_size(px(13.0))
                    .text_color(self.desktop_theme.secondary)
                    .child(format!("/ {project}")),
            );
        }
        identity.child(
            div()
                .min_w(px(0.0))
                .truncate()
                .whitespace_nowrap()
                .text_size(px(14.0))
                .text_color(self.desktop_theme.secondary)
                .child(format!("/ {}", self.desktop_route_title())),
        )
    }

    fn select_sidebar_tab(&mut self, editor: bool, cx: &mut Context<Self>) {
        self.sidebar_editor = editor;
        match (editor, self.route().clone()) {
            (true, NativeRoute::Thread { project, thread }) => {
                self.navigate(
                    NativeRoute::Editor {
                        project,
                        thread,
                        file: None,
                    },
                    cx,
                );
            }
            (
                false,
                NativeRoute::Editor {
                    project, thread, ..
                },
            ) => {
                self.navigate(NativeRoute::Thread { project, thread }, cx);
            }
            _ => cx.notify(),
        }
    }

    fn desktop_sidebar(&mut self, cx: &mut Context<Self>) -> Div {
        let theme = self.desktop_theme;
        let weak = cx.entity().downgrade();
        let tabs = Tabs::new(
            "artisan-workspace-tabs",
            self.sidebar_tabs_focus.clone(),
            self.theme,
            if self.sidebar_editor {
                "editor"
            } else {
                "agents"
            },
            [
                TabSpec::new("agents", "Agents"),
                TabSpec::new("editor", "Editor"),
            ],
        )
        .w_full()
        .rounded(px(6.0))
        .debug_selector("artisan-workspace-tabs")
        .on_change(move |value, _, _, cx| {
            let _ = weak.update(cx, |app, cx| {
                app.select_sidebar_tab(value.as_ref() == "editor", cx)
            });
        });
        let mut nav_theme = theme;
        nav_theme.secondary = self.theme.colors.muted_foreground.to_paint();
        let nav = div()
            .id("artisan-workspace-navigation")
            .track_focus(&self.sidebar_navigation_focus)
            .tab_index(0)
            .w_full()
            .h(px(34.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .hover(|style| style.bg(theme.selected))
            .debug_selector(|| "artisan-workspace-navigation".to_owned())
            .child(desktop_nav_glyph(
                if self.sidebar_editor {
                    AssetId::TABLER_FOLDER_PLUS
                } else {
                    AssetId::TABLER_EDIT
                },
                nav_theme,
            ))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.foreground)
                    .child(if self.sidebar_editor {
                        "Open project"
                    } else {
                        "New thread"
                    }),
            )
            .on_click(cx.listener(|app, _, window, cx| {
                window.focus(&app.sidebar_navigation_focus, cx);
                if app.sidebar_editor {
                    app.activate_add_project(cx);
                } else {
                    app.begin_new_task(cx);
                }
            }))
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    if app.sidebar_editor {
                        app.activate_add_project(cx);
                    } else {
                        app.begin_new_task(cx);
                    }
                }
            }));
        div()
            .h_full()
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(10.0))
            .child(tabs)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(nav)
                    .when(!self.sidebar_editor, |navigation| {
                        navigation.child(
                            div()
                                .id("artisan-marketplace-navigation")
                                .w_full()
                                .h(px(34.0))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .px(px(8.0))
                                .rounded(px(6.0))
                                .debug_selector(|| "artisan-marketplace-navigation".to_owned())
                                .child(desktop_nav_glyph(AssetId::TABLER_SHOPPING_BAG, nav_theme))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .text_color(theme.foreground)
                                        .child("Marketplace"),
                                ),
                        )
                    }),
            )
            .child(div().flex_1().min_h(px(0.0)))
            .child(self.desktop_profile(cx))
    }

    fn activate_profile_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for action in self.profile_menu.take_actions() {
            match action.item_id().as_ref() {
                "settings" => self.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::Models,
                        engine: None,
                    },
                    cx,
                ),
                "add-project" => self.activate_add_project(cx),
                _ => {}
            }
        }
        window.focus(&self.profile_focus, cx);
        cx.notify();
    }

    fn desktop_profile(&self, cx: &Context<Self>) -> Div {
        let theme = self.desktop_theme;
        let origin = self.profile_origin.clone();
        let avatar_theme = self.theme;
        let name = self.profile_name.clone();
        let hostname = self.profile_hostname.clone();
        let fallback = move || {
            crate::shell::profile_avatar(
                &avatar_theme,
                crate::shell::RailIdentity::new(name.as_deref(), hostname.as_deref()),
            )
            .rounded(px(8.0))
            .overflow_hidden()
            .into_any_element()
        };
        let avatar = if let Some(path) = self.profile_picture.clone() {
            gpui::img(path)
                .size(px(32.0))
                .rounded(px(8.0))
                .object_fit(gpui::ObjectFit::Cover)
                .with_fallback(fallback.clone())
                .with_loading(fallback)
                .into_any_element()
        } else {
            fallback()
        };
        let trigger = div()
            .id("artisan-desktop-profile-trigger")
            .debug_selector(|| "artisan-desktop-profile-trigger".to_string())
            .track_focus(&self.profile_focus)
            .tab_index(0)
            .cursor_pointer()
            .rounded(px(8.0))
            .w_full()
            .h(px(44.0))
            .p(px(5.0))
            .border_1()
            .border_color(theme.line)
            .bg(if self.profile_menu.is_open() {
                theme.selected
            } else {
                theme.field
            })
            .hover(move |style| style.bg(theme.selected))
            .focus(move |style| style.border_color(theme.secondary))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .size(px(32.0))
                    .flex_shrink_0()
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .child(avatar),
            )
            .children((!self.sidebar_collapsed).then(|| {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(0.0))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.0))
                            .line_height(px(16.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(self.profile_name.clone().unwrap_or_else(|| "User".into())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.0))
                            .line_height(px(14.0))
                            .text_color(theme.secondary)
                            .child(
                                self.profile_hostname
                                    .clone()
                                    .unwrap_or_else(|| "This computer".into()),
                            ),
                    )
            }))
            .children((!self.sidebar_collapsed).then(|| {
                desktop_nav_glyph(
                    if self.profile_menu.is_open() {
                        AssetId::TABLER_CHEVRON_DOWN
                    } else {
                        AssetId::TABLER_CHEVRON_UP
                    },
                    theme,
                )
            }))
            .on_click(cx.listener(|app, _, window, cx| {
                cx.stop_propagation();
                let _ = app.profile_menu.press_trigger();
                window.focus(&app.profile_focus, cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        let _ = app.profile_menu.dismiss();
                    }
                    "down" => {
                        app.profile_menu.set_open(true);
                        let _ = app.profile_menu.move_next();
                    }
                    "up" => {
                        app.profile_menu.set_open(true);
                        let _ = app.profile_menu.move_previous();
                    }
                    "home" => {
                        let _ = app.profile_menu.move_first();
                    }
                    "end" => {
                        let _ = app.profile_menu.move_last();
                    }
                    "enter" | "space" => {
                        if app.profile_menu.is_open() {
                            let _ = app.profile_menu.activate_highlighted();
                            app.activate_profile_selection(window, cx);
                        } else {
                            let _ = app.profile_menu.press_trigger();
                        }
                    }
                    "tab" => {
                        let _ = app.profile_menu.dismiss();
                        cx.notify();
                        return;
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }));
        let mut root =
            div()
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
        if self.profile_menu.is_open() {
            let mut panel = div()
                .id("artisan-desktop-profile-menu")
                .debug_selector(|| "artisan-desktop-profile-menu".to_string())
                .w(px(248.0))
                .p(px(6.0))
                .rounded(px(12.0))
                .bg(theme.field)
                .border_1()
                .border_color(theme.line)
                .flex()
                .flex_col()
                .block_mouse_except_scroll()
                .on_mouse_down_out(cx.listener(|app, event: &gpui::MouseDownEvent, _, cx| {
                    let trigger = app.profile_origin.get();
                    if !trigger.contains(&event.position) {
                        let _ = app.profile_menu.dismiss();
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .px(px(10.0))
                        .py(px(10.0))
                        .flex()
                        .flex_col()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(13.0))
                                .text_color(theme.foreground)
                                .child(
                                    self.profile_name
                                        .clone()
                                        .unwrap_or_else(|| "This computer".into()),
                                ),
                        )
                        .children(
                            self.profile_hostname
                                .clone()
                                .map(|hostname| desktop_muted(theme, hostname)),
                        ),
                )
                .child(div().h(px(1.0)).bg(theme.line).my(px(4.0)));
            for (index, (label, icon)) in [
                ("Settings", AssetId::TABLER_SETTINGS),
                ("Add project", AssetId::TABLER_FOLDER_PLUS),
            ]
            .into_iter()
            .enumerate()
            {
                panel = panel.child(
                    div()
                        .id(("artisan-profile-action", index))
                        .h(px(34.0))
                        .px(px(10.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .bg(if self.profile_menu.highlighted_index() == Some(index) {
                            theme.selected
                        } else {
                            theme.field
                        })
                        .hover(|style| style.bg(theme.selected))
                        .child(desktop_nav_glyph(icon, theme))
                        .child(desktop_muted(theme, label))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            cx.stop_propagation();
                            let _ = app.profile_menu.activate_index(index);
                            app.activate_profile_selection(window, cx);
                        })),
                );
            }
            root = root.child(gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Anchor::BottomLeft)
                    .position(self.profile_origin.get().origin)
                    .offset(gpui::point(px(0.0), px(-10.0)))
                    .child(panel),
            ));
        }
        root
    }

    fn desktop_route_title(&self) -> String {
        if self.sidebar_editor && matches!(self.route(), NativeRoute::NewThread { .. }) {
            return "Editor".into();
        }
        let (title, _) = match self.route() {
            NativeRoute::NewThread { .. } => ("New task".to_owned(), self.selected_project_name()),
            NativeRoute::Thread { thread, .. } => {
                let title = self
                    .thread_listing
                    .as_ref()
                    .and_then(|listing| {
                        listing
                            .threads()
                            .iter()
                            .find(|item| &item.thread_id == thread)
                    })
                    .map(|item| item.title.as_str().to_owned())
                    .unwrap_or_else(|| "Task".to_owned());
                (title, self.selected_project_name())
            }
            NativeRoute::Editor { .. } => ("Files".to_owned(), self.selected_project_name()),
            NativeRoute::Settings { section, .. } => {
                (format!("Settings / {}", section.as_str()), None)
            }
            NativeRoute::Onboarding => ("Welcome".to_owned(), None),
        };
        title
    }

    fn desktop_route_body(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.sidebar_editor && matches!(self.route(), NativeRoute::NewThread { .. }) {
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(8.0))
                .text_color(self.desktop_theme.secondary)
                .debug_selector(|| "artisan-editor-empty".into())
                .child("Open a project to start editing")
                .into_any_element();
        }
        let route = self.route().clone();
        let route_selector = route.selector_suffix();
        let content = self.route_surface(cx);
        let mut body = div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .debug_selector(move || route_selector.clone());
        if matches!(route, NativeRoute::NewThread { .. }) {
            body = body.child(div().flex_1().min_w(px(0.0)).min_h(px(0.0)).child(content));
            body = body.child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .px(px(24.0))
                    .pt(px(12.0))
                    .pb(px(18.0))
                    .flex()
                    .justify_center()
                    .debug_selector(|| DESKTOP_COMPOSER_SELECTOR.to_string())
                    .child(div().w_full().max_w(px(768.0)).child(self.composer.clone())),
            );
        } else {
            body = body.child(content);
        }
        body.into_any_element()
    }

    /// Returns the host entity when a real thread is selected.
    #[must_use]
    pub fn conversation_host(&self) -> Option<&Entity<ConversationHost>> {
        self.conversation_host.as_ref()
    }

    fn message_submission_is_admissible(&self, cx: &App) -> bool {
        matches!(self.route(), NativeRoute::Thread { project, thread }
            if self.selected_project.as_ref() == Some(project)
                && self.selected_thread.as_ref() == Some(thread))
            && self.message_composer_visible(cx)
    }

    fn project_picker_action_is_admissible(&self) -> bool {
        !self.shutdown_prepared
            && !self.service_stopped
            && self.intake_stage.is_none()
            && self.thread_switch_flight.is_none()
            && self.ordinary_unsubscribe_thread.is_none()
    }

    fn command_submission_is_available(&self) -> bool {
        #[cfg(test)]
        if self.test_command_sink.is_some() {
            return true;
        }
        self.service
            .as_ref()
            .is_some_and(|service| !service.is_finished())
    }

    fn add_project_action_is_admissible(&self) -> bool {
        self.project_picker_action_is_admissible() && self.command_submission_is_available()
    }

    fn message_composer_visible(&self, cx: &App) -> bool {
        let Some(selected_thread) = self.selected_thread.as_ref() else {
            return false;
        };
        self.conversation_host.as_ref().is_some_and(|host| {
            host.read(cx).controller_view().delivery.thread_id == *selected_thread
        }) && matches!(&self.state, NativeViewState::Ready)
            && self.intake_stage.is_none()
            && self.thread_switch_flight.is_none()
            && self.ordinary_unsubscribe_thread.is_none()
            && self.command_submission_is_available()
            && !self.service_stopped
    }

    fn sync_composer_controls(&mut self, cx: &mut Context<Self>) {
        let mut snapshot = self.composer_controls.read(cx).snapshot().clone();
        snapshot.send_ready =
            self.message_submission_is_admissible(cx) && self.composer.read(cx).send_ready();
        snapshot.disabled = self.service_stopped;
        self.project_run_controls(&mut snapshot);
        crate::native_composer_queue::project_controls_snapshot(&self.composer_queue.state, &mut snapshot);
        let queue = &self.composer_queue.state;
        let count = crate::native_composer_queue::queue_count_label(queue);
        let status = queue.status().label();
        snapshot.queue_status = if status.is_empty() { count } else { Some(match count { Some(count) => format!("{count} · {status}"), None => status.to_owned() }) };
        snapshot.queue_retry = if queue.can_retry_restore() { Some("Restore".into()) } else if queue.can_retry_recalled_read() || matches!(queue.status(), crate::composer_queue_state::QueueStatus::TransportFailed) { Some("Retry".into()) } else { None };

        snapshot.new_thread_ready =
            snapshot.run_active && snapshot.send_ready && self.add_project_action_is_admissible();
        snapshot.failure = self.message_failure.map(|notice| {
            crate::native_composer_controls::NativeComposerFailure::new(
                notice.id,
                "Could not send message",
                "Your draft is preserved. Check the connection and try again.",
                self.message_retry
                    .as_ref()
                    .is_some_and(|retry| retry.draft_matches)
                    && self.command_submission_is_available(),
            )
        });
        self.composer_controls
            .update(cx, |controls, cx| controls.set_snapshot(snapshot, cx));
    }

    fn sync_composer_availability(&mut self, cx: &mut Context<Self>) {
        let image_thread = self
            .conversation_host
            .as_ref()
            .map(|host| host.read(cx).controller_view().delivery.thread_id);
        if self
            .message_images
            .update(cx, |images, cx| images.set_current_thread(image_thread, cx))
            .is_err()
        {
            self.state = NativeViewState::Failure(invalid_service_failure());
        }
        let disabled = !self.message_submission_is_admissible(cx);
        let model_label = self
            .engine_settings
            .authoritative_config()
            .map(|config| {
                let artisan_domain::EngineSelection::OpenCode2(selection) = config.selection();
                selection.model_id().as_str().to_owned()
            })
            .unwrap_or_else(|| "Select model".into());
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_attachment_delivery_enabled(true, composer_cx);
            composer.set_surface(disabled, model_label, composer_cx);
        });
        self.sync_composer_controls(cx);
        self.schedule_run_observation(cx);
        self.schedule_composer_queue(false, cx);
    }

    fn begin_message_submission(&mut self, cx: &mut Context<Self>) {
        if !self.message_submission_is_admissible(cx) || self.message_flight.is_some() {
            return;
        }
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        self.clear_message_retry();
        let submission = self
            .composer
            .update(cx, |composer, _| composer.begin_payload_submission());
        let (body, token) = match submission {
            Ok(submission) => submission,
            Err(blocked) => {
                if let Some(failure) = submission_blocked_failure(blocked) {
                    self.message_failure = Some(NativeMessageFailure::new(failure));
                }
                cx.notify();
                return;
            }
        };
        self.message_receipt = None;
        self.message_failure = None;
        let request_id = match create_message_request_id() {
            Ok(request_id) => request_id,
            Err(failure) => {
                self.reject_message_submission(token, failure, cx);
                return;
            }
        };
        let command =
            NativeTransportCommand::QueueMessage(Box::new(artisan_domain::QueueMessage {
                request_id: request_id.clone(),
                thread_id: thread_id.clone(),
                payload: body.clone(),
            }));
        match self.submit_command(command) {
            Ok(()) => {
                self.message_flight = Some(NativeMessageFlight {
                    thread_id,
                    request_id,
                    payload: body,
                    token,
                });
            }
            Err(error) => {
                self.reject_message_submission(token, command_failure(error), cx);
            }
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn message_retry_context_is_admissible(&self, cx: &App) -> bool {
        let Some(retry) = self.message_retry.as_ref() else {
            return false;
        };
        self.selected_thread.as_ref() == Some(&retry.thread_id)
            && self.message_flight.is_none()
            && self.message_submission_is_admissible(cx)
            && !self.composer.read(cx).is_submitting()
    }

    fn message_retry_is_admissible(&self, cx: &App) -> bool {
        self.message_retry_context_is_admissible(cx)
            && self
                .message_retry
                .as_ref()
                .is_some_and(|retry| retry.draft_matches)
    }

    fn observe_composer_change(
        &mut self,
        composer: &Entity<NativeComposer>,
        cx: &mut Context<Self>,
    ) {
        self.sync_composer_controls(cx);
        let Some(retry) = self.message_retry.as_mut() else {
            return;
        };
        if self.message_flight.is_some() {
            return;
        }
        let matches = composer.read(cx).draft_matches_payload(&retry.payload);
        retry.draft_matches = matches;
        cx.notify();
    }

    fn clear_message_retry(&mut self) {
        self.message_retry = None;
        self.message_retry_focus_handle = self.message_retry_focus_handle.clone().tab_stop(false);
    }

    fn activate_message_retry(&mut self, cx: &mut Context<Self>) {
        if self.service_stopped || !self.command_submission_is_available() {
            self.service_stopped = true;
            self.set_picker_disabled(true, cx);
            self.set_thread_picker_disabled(true, cx);
            self.set_failure(command_failure(CommandSendError::Stopped), cx);
            return;
        }
        if !self.message_retry_context_is_admissible(cx) {
            return;
        }
        let Some(retry) = self.message_retry.as_ref() else {
            return;
        };
        let thread_id = retry.thread_id.clone();
        let request_id = retry.request_id.clone();
        let retry_body = retry.payload.clone();
        let submission = self
            .composer
            .update(cx, |composer, _| composer.begin_payload_submission());
        let (body, token) = match submission {
            Ok(submission) => submission,
            Err(blocked) => {
                if let Some(retry) = self.message_retry.as_mut() {
                    retry.draft_matches = false;
                }
                if let Some(failure) = submission_blocked_failure(blocked) {
                    self.message_failure = Some(NativeMessageFailure::new(failure));
                }
                cx.notify();
                return;
            }
        };
        if body != retry_body {
            if let Some(retry) = self.message_retry.as_mut() {
                retry.draft_matches = false;
            }
            self.finish_composer_submission(token, DraftDisposition::Retained, cx);
            self.sync_composer_availability(cx);
            cx.notify();
            return;
        }

        self.message_receipt = None;
        self.message_failure = None;
        let command =
            NativeTransportCommand::QueueMessage(Box::new(artisan_domain::QueueMessage {
                request_id: request_id.clone(),
                thread_id: thread_id.clone(),
                payload: retry_body.clone(),
            }));
        match self.submit_command(command) {
            Ok(()) => {
                self.clear_message_retry();
                self.message_flight = Some(NativeMessageFlight {
                    thread_id,
                    request_id,
                    payload: retry_body,
                    token,
                });
            }
            Err(CommandSendError::Busy) => {
                self.finish_composer_submission(token, DraftDisposition::Retained, cx);
                self.message_failure = Some(NativeMessageFailure::new(command_failure(
                    CommandSendError::Busy,
                )));
            }
            Err(CommandSendError::Stopped) => {
                self.finish_composer_submission(token, DraftDisposition::Retained, cx);
                self.clear_message_retry();
                self.service_stopped = true;
                self.set_picker_disabled(true, cx);
                self.set_thread_picker_disabled(true, cx);
                self.set_failure(command_failure(CommandSendError::Stopped), cx);
                return;
            }
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn finish_composer_submission(
        &mut self,
        token: SubmissionToken,
        disposition: DraftDisposition,
        cx: &mut Context<Self>,
    ) {
        self.composer.update(cx, |composer, composer_cx| {
            composer.finish_submission(token, disposition, composer_cx);
        });
    }

    fn reject_message_submission(
        &mut self,
        token: SubmissionToken,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        self.finish_composer_submission(token, DraftDisposition::Retained, cx);
        self.message_failure = Some(NativeMessageFailure::new(failure));
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn retain_message_flight(&mut self, cx: &mut Context<Self>) {
        if let Some(flight) = self.message_flight.take() {
            self.finish_composer_submission(flight.token, DraftDisposition::Retained, cx);
        }
        self.clear_message_retry();
        self.sync_composer_availability(cx);
    }

    fn clear_message_presentation(&mut self) {
        self.clear_message_retry();
        self.message_receipt = None;
        self.message_failure = None;
    }

    fn handle_message_receipt(&mut self, receipt: QueueMessageReceipt, cx: &mut Context<Self>) {
        let Some(flight) = self.message_flight.as_ref() else {
            return;
        };
        if self.selected_thread.as_ref() != Some(&flight.thread_id)
            || receipt.thread_id != flight.thread_id
            || receipt.request_id != flight.request_id
            || !matches!(
                receipt.disposition,
                artisan_domain::ReceiptDisposition::Accepted
                    | artisan_domain::ReceiptDisposition::Duplicate
            )
        {
            return;
        }
        let flight = self
            .message_flight
            .take()
            .expect("flight was checked above");
        self.finish_composer_submission(flight.token, DraftDisposition::Accepted, cx);
        self.message_receipt = Some(receipt);
        self.message_failure = None;
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn handle_message_failure(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let matches_active = self.message_flight.as_ref().is_some_and(|flight| {
            &flight.thread_id == thread_id
                && &flight.request_id == request_id
                && self.selected_thread.as_ref() == Some(thread_id)
        });
        if !matches_active {
            return;
        }
        let flight = self
            .message_flight
            .take()
            .expect("flight was checked above");
        self.finish_composer_submission(flight.token, DraftDisposition::Retained, cx);
        self.message_retry = Some(NativeMessageRetry {
            thread_id: flight.thread_id,
            request_id: flight.request_id,
            payload: flight.payload,
            draft_matches: false,
        });
        self.message_receipt = None;
        self.message_failure = Some(NativeMessageFailure::new(failure));
        self.sync_composer_availability(cx);
        cx.notify();
    }

    /// Retains any admitted message before the application starts service
    /// shutdown. This runs on the GPUI application thread.
    pub(crate) fn prepare_shutdown(&mut self, cx: &mut Context<Self>) {
        self.shutdown_prepared = true;
        self.thread_switch_flight = None;
        self.ordinary_unsubscribe_thread = None;
        self.pending_thread = None;
        self.set_picker_disabled(true, cx);
        self.set_thread_picker_disabled(true, cx);
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_disabled(true, composer_cx);
        });
        cx.notify();
    }

    fn poll_service(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(service) = self.service.clone() else {
            return false;
        };
        let mut events = Vec::with_capacity(64);
        loop {
            match service.try_recv() {
                Ok(Some(event)) => events.push(event),
                Ok(None) => break,
                Err(EventReceiveError::Stopped) => {
                    self.retain_message_flight(cx);
                    self.set_failure(
                        ServiceFailure {
                            stage: ServiceFailureStage::EventBridge,
                            category: ServiceFailureCategory::ChannelClosed,
                        },
                        cx,
                    );
                    self.service_stopped = true;
                    self.thread_switch_flight = None;
                    self.ordinary_unsubscribe_thread = None;
                    self.pending_thread = None;
                    self.set_picker_disabled(true, cx);
                    self.set_thread_picker_disabled(true, cx);
                    break;
                }
            }
        }
        for event in events {
            self.handle_service_event(event, cx);
        }
        self.retry_thread_switch_if_admitted(cx);
        self.try_mount_pending_thread(cx);
        self.sync_composer_availability(cx);
        !self.service_stopped
    }

    fn handle_service_event(&mut self, event: NativeTransportEvent, cx: &mut Context<Self>) {
        if self.shutdown_prepared || self.service_stopped {
            return;
        }
        match event {
            NativeTransportEvent::ComposerState(event) => self.handle_composer_state_event(event, cx),
            NativeTransportEvent::ActiveRun {
                thread_id,
                generation,
                result,
            } => {
                self.receive_active_run(thread_id, generation, Ok(result), cx);
            }
            NativeTransportEvent::ActiveRunFailed {
                thread_id,
                generation,
                failure,
            } => {
                self.receive_active_run(thread_id, generation, Err(failure), cx);
            }
            NativeTransportEvent::RunStopped(receipt) => self.receive_run_stop(Ok(receipt), cx),
            NativeTransportEvent::StopRunFailed { command, failure } => {
                self.receive_run_stop_failure(command, failure, cx)
            }
            NativeTransportEvent::MessageImageLoaded { reference, image } => {
                self.message_images.update(cx, |images, cx| {
                    images.accept_image(reference, image, cx);
                });
            }
            NativeTransportEvent::MessageImageFailed { reference, failure } => {
                self.message_images.update(cx, |images, cx| {
                    images.fail_image(reference, failure, cx);
                });
            }
            NativeTransportEvent::Starting => {
                self.state = NativeViewState::Loading;
                self.sync_composer_availability(cx);
                cx.notify();
            }
            NativeTransportEvent::Projects(listing) => self.handle_projects(&listing, cx),
            NativeTransportEvent::Threads {
                project_id,
                listing,
            } => self.handle_threads(&project_id, &listing, cx),
            NativeTransportEvent::Snapshot(snapshot) => self.handle_snapshot(snapshot, cx),
            NativeTransportEvent::ProjectIntakeProgress(stage) => {
                self.handle_intake_progress(stage, cx);
            }
            NativeTransportEvent::ProjectIntakeCancelled => self.handle_intake_cancelled(cx),
            NativeTransportEvent::ProjectIntakeReady {
                projects,
                project_id,
                threads,
                thread_id,
            } => self.handle_intake_ready(&projects, project_id, &threads, thread_id, cx),
            NativeTransportEvent::ProjectIntakeFailed {
                operation,
                failure,
                retryable,
            } => self.handle_intake_failed(operation, failure, retryable, cx),
            NativeTransportEvent::EmptyProjects => self.handle_empty_projects(cx),
            NativeTransportEvent::EmptyThreads { project_id } => {
                self.handle_empty_threads(&project_id, cx);
            }
            NativeTransportEvent::Failed(failure) => {
                self.retain_message_flight(cx);
                self.thread_switch_flight = None;
                self.ordinary_unsubscribe_thread = None;
                self.pending_thread = None;
                self.set_picker_disabled(true, cx);
                self.set_thread_picker_disabled(true, cx);
                self.set_failure(failure, cx);
            }
            NativeTransportEvent::ThreadEngineSettings { generation, result } => {
                self.handle_engine_settings(generation, result, cx);
            }
            NativeTransportEvent::RegisteredProfiles(result) => {
                self.handle_registered_profiles(result, cx);
            }
            NativeTransportEvent::RegisteredProfilesFailed(failure) => {
                self.handle_registered_profiles_failed(failure, cx);
            }
            NativeTransportEvent::ComposerCatalog {
                thread_id,
                profile_id,
                generation,
                result,
            } => self.handle_composer_catalog(thread_id, profile_id, generation, result, cx),
            NativeTransportEvent::ComposerCatalogFailed {
                thread_id,
                profile_id,
                generation,
                failure,
            } => {
                self.handle_composer_catalog_failed(thread_id, profile_id, generation, failure, cx)
            }
            NativeTransportEvent::ModelFavorites {
                thread_id,
                profile_id,
                generation,
                result,
            } => self.handle_model_favorites(thread_id, profile_id, generation, result, cx),
            NativeTransportEvent::ModelFavoritesFailed {
                thread_id,
                profile_id,
                generation,
                failure,
            } => self.handle_model_favorites_failed(thread_id, profile_id, generation, failure, cx),
            NativeTransportEvent::ModelFavoriteSet {
                thread_id,
                profile_id,
                request_id,
                receipt,
            } => self.handle_model_favorite_set(thread_id, profile_id, request_id, receipt, cx),
            NativeTransportEvent::ModelFavoriteFailed {
                thread_id,
                profile_id,
                request_id,
                failure,
            } => self.handle_model_favorite_failed(thread_id, profile_id, request_id, failure, cx),
            NativeTransportEvent::ThreadEngineConfigSet(result, retained) => {
                self.handle_engine_config_set(&result, *retained, cx);
            }
            NativeTransportEvent::ThreadEngineConfigConflict {
                thread_id,
                request_id,
            } => {
                self.handle_engine_conflict(thread_id, &request_id, cx);
            }
            NativeTransportEvent::ThreadEngineConfigFailed {
                thread_id,
                request_id,
                failure,
            } => {
                self.handle_engine_config_failed(&thread_id, &request_id, failure, cx);
            }
            NativeTransportEvent::ThreadEngineSettingsFailed {
                thread_id,
                generation,
                failure,
            } => {
                self.handle_engine_settings_failed(thread_id, generation, failure, cx);
            }
            // The shipping composer uses QueueMessage. Legacy first-message results
            // cannot settle a flight from the newer command family.
            NativeTransportEvent::FirstMessageQueued(_)
            | NativeTransportEvent::FirstMessageFailed { .. } => {}
            NativeTransportEvent::MessageQueued(receipt) => {
                self.handle_message_receipt(receipt, cx);
                self.schedule_composer_queue(true, cx);
            }
            NativeTransportEvent::MessageFailed {
                thread_id,
                request_id,
                failure,
            } => {
                self.handle_message_failure(&thread_id, &request_id, failure, cx);
            }
            NativeTransportEvent::ConversationSubscriptionStarted {
                thread_id,
                request_id,
                started,
            } => self.handle_subscription_started(&thread_id, &request_id, started, cx),
            NativeTransportEvent::ConversationSubscriptionStopped {
                thread_id,
                request_id,
                stopped,
            } => self.handle_subscription_stopped(&thread_id, &request_id, &stopped, cx),
            NativeTransportEvent::PatchBatch(batch) => self.handle_patch_batch(&batch, cx),
            NativeTransportEvent::DeliveryLost(failure) => self.handle_delivery_lost(failure, cx),
            NativeTransportEvent::Stopped(status) => self.handle_service_stopped(status, cx),
        }
    }

    fn handle_empty_projects(&mut self, cx: &mut Context<Self>) {
        self.pending_thread = None;
        self.pending_snapshot = None;
        self.thread_listing = None;
        if self.thread_switch_flight.is_none() {
            self.retained_switch_listings.clear();
        }
        self.install_thread_picker(empty_thread_listing(), None, cx);
        if self.thread_switch_flight.is_some() {
            self.handle_removed_thread_during_switch(cx);
        } else {
            self.retire_host(cx);
        }
        self.state = NativeViewState::EmptyProjects;
        cx.notify();
    }

    fn handle_empty_threads(&mut self, project_id: &ProjectId, cx: &mut Context<Self>) {
        if self.selected_project.as_ref() != Some(project_id) {
            self.set_failure(invalid_service_failure(), cx);
            return;
        }
        self.clear_message_retry();
        self.pending_thread = None;
        self.pending_snapshot = None;
        let listing = empty_thread_listing();
        if self.selected_thread.is_some() {
            self.remember_switch_listing();
        }
        self.thread_listing = Some(listing.clone());
        self.update_thread_picker(listing, None, cx);
        if self.thread_switch_flight.is_some() {
            self.handle_removed_thread_during_switch(cx);
        } else if self.conversation_host.is_some() && self.selected_thread.is_some() {
            self.begin_thread_retirement(cx);
        } else {
            self.selected_thread = None;
            self.state = NativeViewState::EmptyThreads;
            self.sync_thread_picker_selected(cx);
            self.engine_settings.select_thread(None);
            self.reset_composer_catalog(cx);
            self.sync_composer_availability(cx);
        }
        cx.notify();
    }

    fn handle_service_stopped(&mut self, status: ServiceStopStatus, cx: &mut Context<Self>) {
        self.retain_message_flight(cx);
        self.service_stopped = true;
        self.reset_composer_catalog(cx);
        self.thread_switch_flight = None;
        self.ordinary_unsubscribe_thread = None;
        self.pending_thread = None;
        self.set_picker_disabled(true, cx);
        self.set_thread_picker_disabled(true, cx);
        if matches!(status, ServiceStopStatus::Failed)
            && !matches!(&self.state, NativeViewState::Failure(_))
        {
            self.set_failure(
                ServiceFailure {
                    stage: ServiceFailureStage::Cleanup,
                    category: ServiceFailureCategory::Cleanup,
                },
                cx,
            );
        } else {
            self.sync_composer_availability(cx);
            cx.notify();
        }
    }

    fn handle_subscription_started(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        started: ConversationSubscriptionStarted,
        cx: &mut Context<Self>,
    ) {
        if self
            .retained_switch_request_ids
            .iter()
            .any(|id| id == request_id)
            || self.active_subscription_request_id.as_ref() == Some(request_id)
        {
            return;
        }

        if self.thread_switch_flight.is_some() {
            self.handle_thread_switch_subscription_started(thread_id, request_id, started, cx);
            return;
        }

        if self.ordinary_unsubscribe_thread.as_ref() == Some(thread_id) {
            self.remember_switch_request_id(request_id.clone());
            return;
        }

        self.handle_standalone_subscription_started(thread_id, request_id, started, cx);
    }

    fn handle_thread_switch_subscription_started(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        started: ConversationSubscriptionStarted,
        cx: &mut Context<Self>,
    ) {
        let Some((target_thread, generation, request_matches)) = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| match &flight.phase {
                ThreadSwitchPhase::AwaitingSubscriptionStart {
                    request_id: receipt,
                } => Some((
                    flight.target_thread.clone(),
                    flight.generation,
                    receipt
                        .as_ref()
                        .is_none_or(|expected| expected == request_id),
                )),
                _ => None,
            })
        else {
            self.remember_switch_request_id(request_id.clone());
            return;
        };
        let Some(target_thread) = target_thread else {
            self.remember_switch_request_id(request_id.clone());
            return;
        };
        if self.shutdown_prepared
            || !request_matches
            || thread_id != &target_thread
            || !self.thread_is_listed(&target_thread)
            || self.selected_thread.as_ref() != Some(&target_thread)
        {
            self.remember_switch_request_id(request_id.clone());
            return;
        }

        let snapshot = match started {
            ConversationSubscriptionStarted::Fresh(start) => start.snapshot().clone(),
            // A switch always submits a fresh subscription with no cursor.
            // A resumed response cannot advance this flight.
            ConversationSubscriptionStarted::Resumed { .. } => {
                self.remember_switch_request_id(request_id.clone());
                return;
            }
        };
        if snapshot.thread_id() != &target_thread {
            self.remember_switch_request_id(request_id.clone());
            return;
        }
        self.advance_thread_switch_with_snapshot(
            &target_thread,
            generation,
            request_id,
            snapshot,
            cx,
        );
    }

    fn advance_thread_switch_with_snapshot(
        &mut self,
        target_thread: &ThreadId,
        generation: u64,
        request_id: &RequestId,
        snapshot: ConversationSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.forget_switch_snapshot_thread(target_thread);
        self.standalone_snapshot_thread = Some(target_thread.clone());
        if let Some(flight) = self.thread_switch_flight.as_mut()
            && flight.generation == generation
            && let ThreadSwitchPhase::AwaitingSubscriptionStart {
                request_id: receipt,
            } = &mut flight.phase
        {
            *receipt = Some(request_id.clone());
        }
        self.remember_switch_request_id(request_id.clone());
        let Some(host) = self.conversation_host.clone() else {
            self.fail_thread_switch(invalid_service_failure(), false, cx);
            return;
        };
        if host.read(cx).controller_view().delivery.thread_id != *target_thread {
            self.fail_thread_switch(invalid_service_failure(), false, cx);
            return;
        }
        self.dispatch_snapshot(&host, snapshot, cx);
        if !self
            .conversation_host
            .as_ref()
            .is_some_and(|mounted| mounted.read(cx).controller_view().delivery.has_snapshot)
        {
            self.fail_thread_switch(invalid_service_failure(), false, cx);
            return;
        }
        let complete = self.thread_switch_flight.as_ref().is_some_and(|flight| {
            flight.generation == generation
                && matches!(
                    &flight.phase,
                    ThreadSwitchPhase::AwaitingSubscriptionStart {
                        request_id: Some(receipt)
                    } if receipt == request_id
                )
                && self.selected_thread.as_ref() == Some(target_thread)
                && self
                    .conversation_host
                    .as_ref()
                    .is_some_and(|mounted| mounted.read(cx).controller_view().delivery.has_snapshot)
        });
        if complete {
            self.active_subscription_request_id = Some(request_id.clone());
            self.thread_switch_flight = None;
            self.pending_thread = None;
            self.sync_thread_picker_selected(cx);
            self.sync_thread_picker_disabled(cx);
            self.sync_composer_availability(cx);
            cx.notify();
        }
    }

    fn handle_standalone_subscription_started(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        started: ConversationSubscriptionStarted,
        cx: &mut Context<Self>,
    ) {
        match started {
            ConversationSubscriptionStarted::Fresh(start) => {
                let snapshot = start.snapshot().clone();
                if thread_id != snapshot.thread_id() {
                    return;
                }
                if self.selected_thread.as_ref() != Some(thread_id) {
                    return;
                }
                self.forget_switch_snapshot_thread(thread_id);
                self.remember_active_subscription_request();
                self.active_subscription_request_id = Some(request_id.clone());
                self.handle_snapshot(snapshot, cx);
                self.standalone_snapshot_thread = Some(thread_id.clone());
            }
            ConversationSubscriptionStarted::Resumed {
                thread_id: resumed_thread,
                cursor,
            } => {
                if self.selected_thread.as_ref() != Some(&resumed_thread)
                    || &resumed_thread != thread_id
                {
                    return;
                }
                self.forget_switch_snapshot_thread(thread_id);
                self.remember_active_subscription_request();
                self.active_subscription_request_id = Some(request_id.clone());
                self.standalone_snapshot_thread = Some(thread_id.clone());
                let Some(host) = self.conversation_host.clone() else {
                    return;
                };
                let dispatch = host.update(cx, |host, host_cx| {
                    host.dispatch(
                        ConversationStateEvent::Delivery(
                            ConversationDeliveryEvent::SubscriptionResumed {
                                thread_id: resumed_thread.clone(),
                                cursor,
                            },
                        ),
                        host_cx,
                    )
                });
                if dispatch.is_err() {
                    self.set_failure(invalid_service_failure(), cx);
                } else {
                    self.acknowledge_host_cursor(&host, cx);
                    self.pump_host_boundary(&host, cx);
                    cx.notify();
                }
            }
        }
    }

    fn handle_subscription_stopped(
        &mut self,
        thread_id: &ThreadId,
        request_id: &RequestId,
        stopped: &artisan_protocol::ConversationSubscriptionStopped,
        cx: &mut Context<Self>,
    ) {
        // A stale ack is ignored: unknown requests return here without
        // submitting Unsubscribe, and no host retirement runs on this path.
        let known_request = self
            .retained_switch_request_ids
            .iter()
            .any(|id| id == request_id);
        if known_request {
            return;
        }
        if &stopped.thread_id != thread_id {
            if self.thread_switch_flight.is_some() {
                self.remember_switch_request_id(request_id.clone());
            }
            return;
        }

        if self.thread_switch_flight.is_some()
            && !self.thread_switch_flight.as_ref().is_some_and(|flight| {
                matches!(
                    &flight.phase,
                    ThreadSwitchPhase::AwaitingUnsubscribeStop { .. }
                )
            })
        {
            self.remember_switch_request_id(request_id.clone());
            return;
        }

        if let Some((source_thread, generation, request_matches)) = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| match &flight.phase {
                ThreadSwitchPhase::AwaitingUnsubscribeStop {
                    request_id: receipt,
                } => Some((
                    flight.source_thread.clone(),
                    flight.generation,
                    receipt
                        .as_ref()
                        .is_none_or(|expected| expected == request_id),
                )),
                _ => None,
            })
        {
            if self.shutdown_prepared || !request_matches || &source_thread != thread_id {
                self.remember_switch_request_id(request_id.clone());
                return;
            }
            if let Some(flight) = self.thread_switch_flight.as_mut()
                && flight.generation == generation
                && let ThreadSwitchPhase::AwaitingUnsubscribeStop {
                    request_id: receipt,
                } = &mut flight.phase
            {
                *receipt = Some(request_id.clone());
            }
            self.remember_switch_request_id(request_id.clone());
            if let Some(flight) = self.thread_switch_flight.as_mut()
                && flight.generation == generation
            {
                flight.phase = ThreadSwitchPhase::HostRetirement {
                    request_id: request_id.clone(),
                };
            }
            self.finish_thread_switch_after_stop(generation, cx);
            return;
        }

        if self.ordinary_unsubscribe_thread.as_ref() != Some(thread_id)
            || self.selected_thread.as_ref() != Some(thread_id)
        {
            return;
        }
        self.ordinary_unsubscribe_thread = None;
        self.remember_switch_request_id(request_id.clone());
        self.remember_active_subscription_request();
        // Stop ack must never create an unsubscribe loop. If this thread is not the active
        // selected thread, it is a stale ack and is ignored. If it is active, finish the
        // already-started local retirement without sending another Unsubscribe.
        // Finish retirement without sending Unsubscribe again
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        if let Some(host) = self.conversation_host.clone() {
            self.pump_host_boundary(&host, cx);
            if self.conversation_effects.is_empty()
                && host.read(cx).total_pending_effect_count() == 0
            {
                Self::release_transient_scroll_custody(&host, cx);
                self.conversation_host = None;
                drop(self.conversation_host_subscription.take());
                self.selected_thread = None;
                self.standalone_snapshot_thread = None;
                self.sync_thread_picker_selected(cx);
                self.sync_thread_picker_disabled(cx);
                self.engine_settings.select_thread(None);
                self.reset_composer_catalog(cx);
                self.sync_composer_availability(cx);
                cx.notify();
                return;
            }
        } else {
            self.selected_thread = None;
            self.standalone_snapshot_thread = None;
            self.sync_thread_picker_selected(cx);
            self.sync_thread_picker_disabled(cx);
            self.engine_settings.select_thread(None);
            self.reset_composer_catalog(cx);
            self.sync_composer_availability(cx);
            cx.notify();
        }
        // If host still has pending effects, keep selected_thread until drained; do not loop
        cx.notify();
    }

    fn handle_patch_batch(&mut self, batch: &PatchBatch, cx: &mut Context<Self>) {
        if self.thread_switch_flight.is_some() {
            self.remember_patch_ids(batch);
            return;
        }
        if self.retained_switch_patch_ids.iter().any(|patch_id| {
            batch
                .patches()
                .iter()
                .any(|patch| patch.patch_id() == patch_id)
        }) {
            return;
        }
        if self.selected_thread.as_ref() != Some(batch.thread_id()) {
            return;
        }
        self.remember_patch_ids(batch);
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        if host.read(cx).controller_view().delivery.thread_id != *batch.thread_id() {
            return;
        }
        let dispatch = host.update(cx, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::BatchReceived(
                    batch.clone(),
                )),
                host_cx,
            )
        });
        if dispatch.is_err() {
            self.set_failure(invalid_service_failure(), cx);
        } else {
            self.acknowledge_host_cursor(&host, cx);
            self.pump_host_boundary(&host, cx);
            cx.notify();
        }
    }

    /// Applies a listing removal to an in-flight switch. A target that has
    /// not been mounted is simply tombstoned; a target whose subscription
    /// admission already reached the service becomes the new mounted source
    /// of one fenced retirement. This keeps the old host until its matching
    /// stop receipt and never lets a removed target start replace it.
    fn handle_removed_thread_during_switch(&mut self, cx: &mut Context<Self>) {
        let Some(flight) = self.thread_switch_flight.as_ref() else {
            return;
        };
        let Some(target_thread) = flight.target_thread.clone() else {
            return;
        };
        let target_subscribe_is_only_retained_retry = matches!(
            &flight.phase,
            ThreadSwitchPhase::SubscribeAdmission {
                retry_pending: true,
                ..
            }
        );
        let retire_mounted_target = matches!(
            &flight.phase,
            ThreadSwitchPhase::SubscribeAdmission { .. }
                | ThreadSwitchPhase::AwaitingSubscriptionStart { .. }
        ) && self.conversation_host.as_ref().is_some_and(|host| {
            self.selected_thread.as_ref() == Some(&target_thread)
                && host.read(cx).controller_view().delivery.thread_id == target_thread
        });

        if retire_mounted_target {
            if target_subscribe_is_only_retained_retry {
                self.thread_switch_flight = None;
                self.pending_thread = None;
                self.retire_host_after_switch_stop(cx);
                self.state = if self.thread_listing.is_none() || self.project_options.is_empty() {
                    NativeViewState::EmptyProjects
                } else {
                    NativeViewState::EmptyThreads
                };
                self.sync_thread_picker_selected(cx);
                self.sync_thread_picker_disabled(cx);
                self.sync_composer_availability(cx);
                cx.notify();
                return;
            }
            self.thread_switch_flight = None;
            self.pending_thread = None;
            self.begin_thread_transition(None, target_thread, cx);
            return;
        }

        let subscribe_generation = self.thread_switch_flight.as_ref().and_then(|flight| {
            matches!(&flight.phase, ThreadSwitchPhase::SubscribeAdmission { .. })
                .then_some(flight.generation)
        });
        if let Some(flight) = self.thread_switch_flight.as_mut() {
            flight.target_thread = None;
        }
        if let Some(generation) = subscribe_generation {
            self.pending_thread = None;
            self.complete_thread_retirement(generation, cx);
        }
    }

    fn thread_is_listed(&self, thread_id: &ThreadId) -> bool {
        self.thread_listing.as_ref().is_some_and(|listing| {
            listing.threads().iter().any(|thread| {
                &thread.thread_id == thread_id
                    && self.selected_project.as_ref() == Some(&thread.project_id)
            })
        })
    }

    fn remember_switch_request_id(&mut self, request_id: RequestId) {
        if self
            .retained_switch_request_ids
            .iter()
            .any(|retained| retained == &request_id)
        {
            return;
        }
        if self.retained_switch_request_ids.len() >= MAX_RETAINED_SWITCH_REQUEST_IDS {
            self.retained_switch_request_ids.remove(0);
        }
        self.retained_switch_request_ids.push(request_id);
    }

    fn remember_active_subscription_request(&mut self) {
        if let Some(request_id) = self.active_subscription_request_id.take() {
            self.remember_switch_request_id(request_id);
        }
    }

    fn remember_patch_ids(&mut self, batch: &PatchBatch) {
        for patch in batch.patches() {
            if self
                .retained_switch_patch_ids
                .iter()
                .any(|retained| retained == patch.patch_id())
            {
                continue;
            }
            if self.retained_switch_patch_ids.len() >= MAX_RETAINED_SWITCH_PATCH_IDS {
                self.retained_switch_patch_ids.remove(0);
            }
            self.retained_switch_patch_ids
                .push(patch.patch_id().clone());
        }
    }

    fn remember_switch_listing(&mut self) {
        let Some(listing) = self.thread_listing.clone() else {
            return;
        };
        if self
            .retained_switch_listings
            .iter()
            .any(|retained| retained == &listing)
        {
            return;
        }
        if self.retained_switch_listings.len() >= MAX_RETAINED_SWITCH_LISTINGS {
            self.retained_switch_listings.remove(0);
        }
        self.retained_switch_listings.push(listing);
    }

    fn remember_switch_snapshot_thread(&mut self, thread_id: ThreadId) {
        if self
            .retained_switch_snapshot_threads
            .iter()
            .any(|retained| retained == &thread_id)
        {
            return;
        }
        if self.retained_switch_snapshot_threads.len() >= MAX_RETAINED_SWITCH_REQUEST_IDS {
            self.retained_switch_snapshot_threads.remove(0);
        }
        self.retained_switch_snapshot_threads.push(thread_id);
    }

    fn forget_switch_snapshot_thread(&mut self, thread_id: &ThreadId) {
        self.retained_switch_snapshot_threads
            .retain(|retained| retained != thread_id);
    }

    fn sync_thread_picker_selected(&mut self, cx: &mut Context<Self>) {
        let Some(picker) = self.thread_picker.clone() else {
            return;
        };
        let selected = self.selected_thread.clone();
        picker.update(cx, |picker, picker_cx| {
            picker.set_selected_thread(selected, picker_cx);
        });
    }

    fn begin_thread_switch(&mut self, target_thread: ThreadId, cx: &mut Context<Self>) {
        if self.shutdown_prepared
            || self.service_stopped
            || self.intake_stage.is_some()
            || self.thread_switch_flight.is_some()
            || self.ordinary_unsubscribe_thread.is_some()
        {
            return;
        }
        let Some(source_thread) = self.selected_thread.clone() else {
            return;
        };
        if source_thread == target_thread {
            return;
        }
        if !self.thread_is_listed(&target_thread) {
            self.set_failure(invalid_service_failure(), cx);
            return;
        }
        self.begin_thread_transition(Some(target_thread), source_thread, cx);
    }

    fn begin_thread_retirement(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_prepared
            || self.service_stopped
            || self.intake_stage.is_some()
            || self.thread_switch_flight.is_some()
            || self.ordinary_unsubscribe_thread.is_some()
        {
            return;
        }
        let Some(source_thread) = self.selected_thread.clone() else {
            return;
        };
        self.begin_thread_transition(None, source_thread, cx);
    }

    fn begin_thread_transition(
        &mut self,
        target_thread: Option<ThreadId>,
        source_thread: ThreadId,
        cx: &mut Context<Self>,
    ) {
        if self
            .conversation_host
            .as_ref()
            .is_none_or(|host| host.read(cx).controller_view().delivery.thread_id != source_thread)
        {
            self.set_failure(invalid_service_failure(), cx);
            return;
        }
        let Some(generation) = self.next_thread_switch_generation.checked_add(1) else {
            self.set_failure(invalid_service_failure(), cx);
            return;
        };
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.next_thread_switch_generation = generation;
        self.remember_switch_listing();
        self.remember_switch_snapshot_thread(source_thread.clone());
        if let Some(target_thread) = target_thread.as_ref() {
            self.remember_switch_snapshot_thread(target_thread.clone());
        }
        self.remember_active_subscription_request();
        self.pending_thread = None;
        self.thread_switch_flight = Some(ThreadSwitchFlight {
            source_thread,
            target_thread,
            generation,
            phase: ThreadSwitchPhase::UnsubscribeAdmission {
                retry_pending: false,
                retry_used: false,
            },
        });
        self.sync_thread_picker_disabled(cx);
        self.sync_composer_availability(cx);
        self.submit_thread_switch_unsubscribe(cx);
    }

    fn submit_command(&self, command: NativeTransportCommand) -> Result<(), CommandSendError> {
        #[cfg(test)]
        if let Some(sink) = &self.test_command_sink {
            sink.commands.borrow_mut().push(command);
            return sink.outcomes.borrow_mut().pop_front().unwrap_or(Ok(()));
        }
        let Some(service) = self.service.as_ref() else {
            return Err(CommandSendError::Stopped);
        };
        service.submit(command)
    }

    fn submit_thread_switch_unsubscribe(&mut self, cx: &mut Context<Self>) {
        let Some((source_thread, generation, retry_used)) = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| match &flight.phase {
                ThreadSwitchPhase::UnsubscribeAdmission {
                    retry_pending: _,
                    retry_used,
                } => Some((flight.source_thread.clone(), flight.generation, *retry_used)),
                _ => None,
            })
        else {
            return;
        };
        match self.submit_command(NativeTransportCommand::Unsubscribe {
            thread_id: source_thread,
        }) {
            Ok(()) => {
                if let Some(flight) = self.thread_switch_flight.as_mut()
                    && flight.generation == generation
                {
                    flight.phase = ThreadSwitchPhase::AwaitingUnsubscribeStop { request_id: None };
                }
                cx.notify();
            }
            Err(CommandSendError::Busy) if !retry_used => {
                if let Some(flight) = self.thread_switch_flight.as_mut()
                    && flight.generation == generation
                {
                    flight.phase = ThreadSwitchPhase::UnsubscribeAdmission {
                        retry_pending: true,
                        retry_used: true,
                    };
                }
                cx.notify();
            }
            Err(CommandSendError::Busy) => self.fail_thread_switch(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::Backpressure,
                },
                false,
                cx,
            ),
            Err(CommandSendError::Stopped) => self.fail_thread_switch(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                },
                true,
                cx,
            ),
        }
    }

    fn submit_thread_switch_subscribe(&mut self, cx: &mut Context<Self>) {
        let Some((target_thread, generation, retry_used)) = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| match &flight.phase {
                ThreadSwitchPhase::SubscribeAdmission {
                    retry_pending: _,
                    retry_used,
                } => Some((flight.target_thread.clone(), flight.generation, *retry_used)),
                _ => None,
            })
        else {
            return;
        };
        let Some(target_thread) = target_thread else {
            self.complete_thread_retirement(generation, cx);
            return;
        };
        if self.shutdown_prepared || !self.thread_is_listed(&target_thread) {
            self.complete_thread_retirement(generation, cx);
            return;
        }
        match self.submit_command(NativeTransportCommand::Subscribe {
            thread_id: target_thread,
            after: None,
        }) {
            Ok(()) => {
                if let Some(flight) = self.thread_switch_flight.as_mut()
                    && flight.generation == generation
                {
                    flight.phase =
                        ThreadSwitchPhase::AwaitingSubscriptionStart { request_id: None };
                }
                cx.notify();
            }
            Err(CommandSendError::Busy) if !retry_used => {
                if let Some(flight) = self.thread_switch_flight.as_mut()
                    && flight.generation == generation
                {
                    flight.phase = ThreadSwitchPhase::SubscribeAdmission {
                        retry_pending: true,
                        retry_used: true,
                    };
                }
                cx.notify();
            }
            Err(CommandSendError::Busy) => self.fail_thread_switch(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::Backpressure,
                },
                false,
                cx,
            ),
            Err(CommandSendError::Stopped) => self.fail_thread_switch(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                },
                true,
                cx,
            ),
        }
    }

    fn retry_thread_switch_if_admitted(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_prepared || self.service_stopped {
            return;
        }
        let retry = self.thread_switch_flight.as_ref().is_some_and(|flight| {
            matches!(
                &flight.phase,
                ThreadSwitchPhase::UnsubscribeAdmission {
                    retry_pending: true,
                    ..
                } | ThreadSwitchPhase::SubscribeAdmission {
                    retry_pending: true,
                    ..
                }
            )
        });
        if !retry {
            return;
        }
        match self
            .thread_switch_flight
            .as_ref()
            .map(|flight| &flight.phase)
        {
            Some(ThreadSwitchPhase::UnsubscribeAdmission { .. }) => {
                self.submit_thread_switch_unsubscribe(cx);
            }
            Some(ThreadSwitchPhase::SubscribeAdmission { .. }) => {
                self.submit_thread_switch_subscribe(cx);
            }
            _ => {}
        }
    }

    fn finish_thread_switch_after_stop(&mut self, generation: u64, cx: &mut Context<Self>) {
        let Some((target_thread, stop_request_id)) =
            self.thread_switch_flight.as_ref().and_then(|flight| {
                if flight.generation != generation {
                    return None;
                }
                match &flight.phase {
                    ThreadSwitchPhase::HostRetirement { request_id } => {
                        Some((flight.target_thread.clone(), request_id.clone()))
                    }
                    _ => None,
                }
            })
        else {
            return;
        };
        self.remember_switch_request_id(stop_request_id);
        self.retire_host_after_switch_stop(cx);
        if self.shutdown_prepared {
            self.thread_switch_flight = None;
            return;
        }
        let target_thread = target_thread.filter(|thread_id| self.thread_is_listed(thread_id));
        if let Some(flight) = self.thread_switch_flight.as_mut()
            && flight.generation == generation
        {
            flight.phase = ThreadSwitchPhase::SubscribeAdmission {
                retry_pending: false,
                retry_used: false,
            };
        }
        self.pending_thread = target_thread;
        if self.pending_thread.is_some() {
            self.state = NativeViewState::Loading;
            self.sync_thread_picker_selected(cx);
            self.try_mount_pending_thread(cx);
        } else {
            self.complete_thread_retirement(generation, cx);
        }
    }

    fn complete_thread_retirement(&mut self, generation: u64, cx: &mut Context<Self>) {
        if self
            .thread_switch_flight
            .as_ref()
            .is_none_or(|flight| flight.generation != generation)
        {
            return;
        }
        self.thread_switch_flight = None;
        self.pending_thread = None;
        self.state = if self.thread_listing.is_none() || self.project_options.is_empty() {
            NativeViewState::EmptyProjects
        } else {
            NativeViewState::EmptyThreads
        };
        self.sync_thread_picker_selected(cx);
        self.sync_thread_picker_disabled(cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn retire_host_after_switch_stop(&mut self, cx: &mut Context<Self>) {
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.pending_snapshot = None;
        self.conversation_effects.clear();
        if let Some(host) = self.conversation_host.clone() {
            Self::release_transient_scroll_custody(&host, cx);
        }
        self.conversation_host = None;
        drop(self.conversation_host_subscription.take());
        self.selected_thread = None;
        self.standalone_snapshot_thread = None;
        self.engine_settings.select_thread(None);
        self.reset_composer_catalog(cx);
        self.sync_thread_picker_selected(cx);
        self.sync_composer_availability(cx);
    }

    fn fail_thread_switch(
        &mut self,
        failure: ServiceFailure,
        terminal: bool,
        cx: &mut Context<Self>,
    ) {
        let target_host_mounted = self.thread_switch_flight.as_ref().is_some_and(|flight| {
            matches!(
                &flight.phase,
                ThreadSwitchPhase::SubscribeAdmission { .. }
                    | ThreadSwitchPhase::AwaitingSubscriptionStart { .. }
            ) && flight
                .target_thread
                .as_ref()
                .is_some_and(|target| self.selected_thread.as_ref() == Some(target))
        });
        if target_host_mounted {
            self.retire_host_after_switch_stop(cx);
        }
        self.thread_switch_flight = None;
        self.pending_thread = None;
        if terminal {
            self.service_stopped = true;
        }
        self.set_failure(failure, cx);
        self.sync_thread_picker_disabled(cx);
        cx.notify();
    }

    fn handle_delivery_lost(&mut self, failure: ServiceFailure, cx: &mut Context<Self>) {
        // Use mounted host's last-good cursor and existing recovery policy to resubscribe
        if let Some(thread_id) = self.selected_thread.clone()
            && let Some(host) = self.conversation_host.clone()
        {
            let cursor = host.read(cx).controller_view().delivery.cursor;
            if let Some(service) = self.service.clone() {
                // Explicit retry via Subscribe with last-good cursor; Busy/Stopped remain explicit
                let result = service.submit(NativeTransportCommand::Subscribe {
                    thread_id: thread_id.clone(),
                    after: cursor,
                });
                match result {
                    Ok(()) => {
                        // keep current view, await Started/Patch; do not fabricate snapshot
                        cx.notify();
                        return;
                    }
                    Err(CommandSendError::Busy) => {
                        self.set_failure(
                            ServiceFailure {
                                stage: ServiceFailureStage::EventBridge,
                                category: ServiceFailureCategory::Backpressure,
                            },
                            cx,
                        );
                        return;
                    }
                    Err(CommandSendError::Stopped) => {
                        self.set_failure(
                            ServiceFailure {
                                stage: ServiceFailureStage::EventBridge,
                                category: ServiceFailureCategory::ChannelClosed,
                            },
                            cx,
                        );
                        return;
                    }
                }
            }
        }
        self.set_failure(failure, cx);
    }

    fn handle_intake_progress(&mut self, stage: NativeProjectIntakeStage, cx: &mut Context<Self>) {
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        if self.intake_restore_state.is_none() {
            self.intake_restore_state = Some(self.state.clone());
        }
        self.intake_stage = Some(stage);
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        self.set_picker_disabled(true, cx);
        self.set_thread_picker_disabled(true, cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn handle_intake_cancelled(&mut self, cx: &mut Context<Self>) {
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.intake_stage = None;
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        if let Some(state) = self.intake_restore_state.take() {
            self.state = state;
        }
        let options = self.project_options.clone();
        self.install_picker(options, self.selected_project.clone(), cx);
        self.install_thread_picker(
            self.thread_listing
                .clone()
                .unwrap_or_else(empty_thread_listing),
            self.selected_thread.clone(),
            cx,
        );
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn handle_intake_failed(
        &mut self,
        operation: NativeProjectIntakeOperation,
        failure: ServiceFailure,
        retryable: bool,
        cx: &mut Context<Self>,
    ) {
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.intake_stage = None;
        self.intake_failure_operation = Some(operation);
        self.intake_retry_available = retryable;
        self.state = NativeViewState::Failure(failure);
        self.last_picker_action = None;
        if !retryable {
            self.intake_restore_state = None;
        }
        // Recreate the public picker so the previous NewProject action
        // cannot be observed as a second retry before the user acts.
        let options = self.project_options.clone();
        self.install_picker(options, self.selected_project.clone(), cx);
        self.install_thread_picker(
            self.thread_listing
                .clone()
                .unwrap_or_else(empty_thread_listing),
            self.selected_thread.clone(),
            cx,
        );
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn handle_intake_ready(
        &mut self,
        projects: &ProjectListing,
        project_id: ProjectId,
        threads: &artisan_domain::ThreadListing,
        thread_id: ThreadId,
        cx: &mut Context<Self>,
    ) {
        if self.thread_switch_flight.is_some() {
            return;
        }
        if !ready_membership_is_valid(projects, &project_id, threads, &thread_id) {
            self.handle_intake_failed(
                NativeProjectIntakeOperation::RefreshThreads,
                invalid_service_failure(),
                false,
                cx,
            );
            return;
        }
        let options = project_options_from_listing(projects);
        let keep_mounted_thread = self.selected_project.as_ref() == Some(&project_id)
            && self.selected_thread.as_ref() == Some(&thread_id)
            && self.conversation_host.as_ref().is_some_and(|host| {
                host.read(cx).controller_view().delivery.thread_id == thread_id
            });
        if self.selected_project.as_ref() != Some(&project_id) {
            self.retained_switch_listings.clear();
        }
        if !keep_mounted_thread {
            self.retire_host(cx);
        }
        self.project_options.clone_from(&options);
        self.selected_project = Some(project_id.clone());
        self.thread_listing = Some(threads.clone());
        if keep_mounted_thread {
            self.pending_thread = None;
        } else {
            self.selected_thread = None;
            self.pending_thread = Some(thread_id);
        }
        self.pending_snapshot = None;
        self.intake_stage = None;
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        self.intake_restore_state = None;
        self.state = if keep_mounted_thread
            && self
                .conversation_host
                .as_ref()
                .is_some_and(|host| host.read(cx).controller_view().delivery.has_snapshot)
        {
            NativeViewState::Ready
        } else {
            NativeViewState::Loading
        };
        self.install_picker(options, Some(project_id), cx);
        self.install_thread_picker(threads.clone(), self.selected_thread.clone(), cx);
        self.try_mount_pending_thread(cx);
        self.sync_command_menu_groups(cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn handle_projects(&mut self, listing: &ProjectListing, cx: &mut Context<Self>) {
        if self.thread_switch_flight.is_some() {
            return;
        }
        let options = project_options_from_listing(listing);
        let selected_project = options.first().map(|project| project.id.clone());
        if self.selected_project != selected_project || selected_project.is_none() {
            self.retire_host(cx);
            self.pending_thread = None;
            self.pending_snapshot = None;
            self.thread_listing = None;
            self.retained_switch_listings.clear();
            self.install_thread_picker(empty_thread_listing(), None, cx);
        }
        self.project_options.clone_from(&options);
        self.selected_project = selected_project;
        self.install_picker(options, self.selected_project.clone(), cx);
        if self.project_options.is_empty() {
            self.state = NativeViewState::EmptyProjects;
        } else {
            self.state = NativeViewState::LoadingThreads;
        }
        self.sync_command_menu_groups(cx);
        cx.notify();
    }

    fn install_picker(
        &mut self,
        options: Vec<ProjectOption>,
        current: Option<ProjectId>,
        cx: &mut Context<Self>,
    ) {
        let picker = cx
            .new(|picker_cx| ProjectPickerView::new(options, current, ThemeMode::Dark, picker_cx));
        let subscription = cx.observe(&picker, |application, picker, cx| {
            application.route_picker_action(&picker, cx);
        });
        self.picker = Some(picker);
        drop(self.picker_subscription.replace(subscription));
        self.last_picker_action = None;
        self.sync_thread_picker_disabled(cx);
    }

    fn install_thread_picker(
        &mut self,
        listing: ThreadListing,
        selected_thread: Option<ThreadId>,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.new(|picker_cx| {
            NativeThreadPicker::new(listing, selected_thread, ThemeMode::Dark, picker_cx)
        });
        let subscription = cx.observe(&picker, |application, picker, cx| {
            application.route_thread_picker_action(&picker, cx);
        });
        self.thread_picker = Some(picker);
        drop(self.thread_picker_subscription.replace(subscription));
        self.sync_thread_picker_disabled(cx);
    }

    fn update_thread_picker(
        &mut self,
        listing: ThreadListing,
        selected_thread: Option<ThreadId>,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.thread_picker.clone() else {
            self.install_thread_picker(listing, selected_thread, cx);
            return;
        };
        picker.update(cx, |picker, picker_cx| {
            picker.replace_listing(listing, picker_cx);
            picker.set_selected_thread(selected_thread, picker_cx);
        });
        self.sync_thread_picker_disabled(cx);
    }

    fn sync_thread_picker_disabled(&mut self, cx: &mut Context<Self>) {
        let disabled = !self.project_picker_action_is_admissible();
        self.set_picker_disabled(disabled, cx);
        self.set_thread_picker_disabled(disabled, cx);
    }

    fn set_picker_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.clone() else {
            return;
        };
        picker.update(cx, |picker, picker_cx| {
            picker.set_disabled(disabled, picker_cx);
        });
    }

    fn set_thread_picker_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        let Some(picker) = self.thread_picker.clone() else {
            return;
        };
        picker.update(cx, |picker, picker_cx| {
            picker.set_disabled(disabled, picker_cx);
        });
    }

    fn route_thread_picker_action(
        &mut self,
        picker: &Entity<NativeThreadPicker>,
        cx: &mut Context<Self>,
    ) {
        let action = picker.update(cx, |picker, _| picker.take_pending_action());
        match action {
            Some(ThreadPickerAction::OpenThread { thread_id }) => {
                self.begin_thread_switch(thread_id, cx);
            }
            None => {}
        }
    }

    fn handle_threads(
        &mut self,
        project_id: &ProjectId,
        listing: &ThreadListing,
        cx: &mut Context<Self>,
    ) {
        let listing_is_valid = self.selected_project.as_ref() == Some(project_id)
            && listing
                .threads()
                .iter()
                .all(|thread| &thread.project_id == project_id);
        if self.thread_switch_flight.is_some() {
            self.handle_threads_during_switch(listing_is_valid, listing, cx);
            self.sync_command_menu_groups(cx);
            return;
        }
        self.handle_threads_without_switch(listing_is_valid, project_id, listing, cx);
        self.sync_command_menu_groups(cx);
    }

    fn handle_threads_during_switch(
        &mut self,
        listing_is_valid: bool,
        listing: &ThreadListing,
        cx: &mut Context<Self>,
    ) {
        if !listing_is_valid {
            // A project/catalog response from an older project selection
            // cannot mutate a newer thread-switch generation.
            return;
        }
        let source_removed = self.thread_switch_flight.as_ref().is_some_and(|flight| {
            !listing
                .threads()
                .iter()
                .any(|thread| thread.thread_id == flight.source_thread)
        });
        let target_removed = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| flight.target_thread.as_ref())
            .is_some_and(|target| {
                !listing
                    .threads()
                    .iter()
                    .any(|thread| &thread.thread_id == target)
            });
        if source_removed || target_removed {
            self.remember_switch_listing();
            self.thread_listing = Some(listing.clone());
            let selected_thread = self.selected_thread.clone();
            self.update_thread_picker(listing.clone(), selected_thread, cx);
            self.pending_snapshot = None;
            if target_removed {
                self.handle_removed_thread_during_switch(cx);
            }
            self.sync_thread_picker_disabled(cx);
            cx.notify();
        }
    }

    fn handle_threads_without_switch(
        &mut self,
        listing_is_valid: bool,
        _project_id: &ProjectId,
        listing: &ThreadListing,
        cx: &mut Context<Self>,
    ) {
        if !listing_is_valid {
            self.set_failure(invalid_service_failure(), cx);
            return;
        }
        if self
            .thread_listing
            .as_ref()
            .is_some_and(|current| current != listing)
            && self
                .retained_switch_listings
                .iter()
                .any(|retained| retained == listing)
        {
            return;
        }
        let selected_removed = self.selected_thread.as_ref().is_some_and(|selected| {
            !listing
                .threads()
                .iter()
                .any(|thread| &thread.thread_id == selected)
        });
        if selected_removed {
            self.remember_switch_listing();
            self.clear_message_retry();
        }
        self.thread_listing = Some(listing.clone());
        let selected_thread = self.selected_thread.clone();
        self.update_thread_picker(listing.clone(), selected_thread.clone(), cx);
        self.pending_snapshot = None;

        let selected_is_listed = selected_thread.as_ref().is_some_and(|selected| {
            listing
                .threads()
                .iter()
                .any(|thread| &thread.thread_id == selected)
        });
        match (selected_thread, selected_is_listed) {
            (Some(selected_thread), true) => {
                self.pending_thread = self
                    .conversation_host
                    .as_ref()
                    .is_some_and(|host| {
                        host.read(cx).controller_view().delivery.thread_id == selected_thread
                    })
                    .then_some(selected_thread);
                if self.pending_thread.is_some() {
                    self.pending_thread = None;
                    if self
                        .conversation_host
                        .as_ref()
                        .is_some_and(|host| host.read(cx).controller_view().delivery.has_snapshot)
                    {
                        self.state = NativeViewState::Ready;
                    }
                } else {
                    self.state = NativeViewState::Loading;
                    self.try_mount_pending_thread(cx);
                }
            }
            (Some(_), false) => {
                self.pending_thread = None;
                self.update_thread_picker(listing.clone(), None, cx);
                if self.conversation_host.is_some() {
                    self.begin_thread_retirement(cx);
                } else {
                    self.selected_thread = None;
                    self.state = NativeViewState::EmptyThreads;
                    self.sync_thread_picker_selected(cx);
                    self.engine_settings.select_thread(None);
                    self.reset_composer_catalog(cx);
                    self.sync_composer_availability(cx);
                }
            }
            (None, _) => {
                self.pending_thread = listing
                    .threads()
                    .first()
                    .map(|thread| thread.thread_id.clone());
                if self.pending_thread.is_none() {
                    self.state = NativeViewState::EmptyThreads;
                } else {
                    self.state = NativeViewState::Loading;
                    self.try_mount_pending_thread(cx);
                }
            }
        }
        self.sync_thread_picker_disabled(cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn handle_snapshot(&mut self, snapshot: ConversationSnapshot, cx: &mut Context<Self>) {
        let thread_id = snapshot.thread_id().clone();
        // A standalone snapshot cannot identify the subscription generation.
        // During a switch only the matching fresh-start payload may advance
        // the target host; every other snapshot is stale until the flight has
        // completed.
        if self.thread_switch_flight.is_some() {
            return;
        }
        if self.standalone_snapshot_thread.as_ref() == Some(&thread_id) {
            return;
        }
        if self
            .retained_switch_snapshot_threads
            .iter()
            .any(|retained| retained == &thread_id)
        {
            return;
        }
        if self.selected_thread.as_ref() != Some(&thread_id) {
            self.pending_snapshot = Some(snapshot);
            if self.pending_thread.as_ref() != Some(&thread_id) {
                self.set_failure(invalid_service_failure(), cx);
            }
            return;
        }
        let Some(host) = self.conversation_host.clone() else {
            self.pending_snapshot = Some(snapshot);
            return;
        };
        self.dispatch_snapshot(&host, snapshot, cx);
    }

    fn dispatch_snapshot(
        &mut self,
        host: &Entity<ConversationHost>,
        snapshot: ConversationSnapshot,
        cx: &mut Context<Self>,
    ) {
        let dispatch = host.update(cx, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::Delivery(ConversationDeliveryEvent::SnapshotReceived(
                    snapshot,
                )),
                host_cx,
            )
        });
        if dispatch.is_err() {
            self.set_failure(invalid_service_failure(), cx);
        } else {
            self.pending_snapshot = None;
            self.state = NativeViewState::Ready;
            self.acknowledge_host_cursor(host, cx);
            self.pump_host_boundary(host, cx);
            self.sync_composer_availability(cx);
            cx.notify();
        }
    }

    fn acknowledge_host_cursor(&mut self, host: &Entity<ConversationHost>, cx: &mut Context<Self>) {
        let delivery = host.read(cx).controller_view().delivery;
        let Some(cursor) = delivery.cursor else {
            self.set_failure(invalid_service_failure(), cx);
            return;
        };
        let Some(service) = self.service.clone() else {
            // Host-only tests deliberately omit the service and therefore have
            // no custody owner to acknowledge.
            return;
        };
        match service.submit(NativeTransportCommand::AcknowledgePatch {
            thread_id: delivery.thread_id,
            cursor,
        }) {
            Ok(()) => {}
            Err(CommandSendError::Busy) => self.set_failure(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::Backpressure,
                },
                cx,
            ),
            Err(CommandSendError::Stopped) => self.set_failure(
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                },
                cx,
            ),
        }
    }

    fn route_picker_action(&mut self, picker: &Entity<ProjectPickerView>, cx: &mut Context<Self>) {
        let Some(action) = picker.read(cx).last_action() else {
            return;
        };
        if self.last_picker_action.as_ref() == Some(&action) {
            return;
        }
        self.last_picker_action = Some(action.clone());
        if !self.project_picker_action_is_admissible() {
            return;
        }
        match picker_route(&action, &self.project_options) {
            Ok(PickerRoute::Select(project_id)) => {
                self.select_project_from_sidebar(project_id, cx);
            }
            Ok(PickerRoute::BeginProjectIntake) => {
                self.submit_intake_command(cx);
            }
            Err(failure) => self.set_failure(failure, cx),
        }
    }

    fn submit_intake_command(&mut self, cx: &mut Context<Self>) {
        let retryable = self.intake_retry_available;
        match self.submit_command(intake_command(retryable)) {
            Ok(()) => {
                self.retain_message_flight(cx);
                self.clear_message_presentation();
                if self.intake_restore_state.is_none() {
                    self.intake_restore_state = Some(self.state.clone());
                }
                self.intake_stage = Some(NativeProjectIntakeStage::PickingDirectory);
                self.intake_failure_operation = None;
                self.intake_retry_available = false;
                self.state = NativeViewState::Loading;
                self.set_picker_disabled(true, cx);
                cx.notify();
            }
            Err(error) => {
                self.handle_intake_failed(
                    NativeProjectIntakeOperation::PickDirectory,
                    command_failure(error),
                    false,
                    cx,
                );
            }
        }
    }

    fn activate_add_project(&mut self, cx: &mut Context<Self>) {
        if !self.add_project_action_is_admissible() {
            return;
        }
        self.submit_intake_command(cx);
    }

    fn add_project_button(&mut self, cx: &mut Context<Self>) -> Button {
        let disabled = !self.add_project_action_is_admissible();
        self.add_project_focus_handle = self.add_project_focus_handle.clone().tab_stop(!disabled);
        let application = cx.entity().downgrade();
        Button::new(
            NATIVE_RAIL_ADD_PROJECT_SELECTOR,
            self.add_project_focus_handle.clone(),
            self.theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(
                AssetId::TABLER_FOLDER_PLUS,
                AccessibleLabel::new(NATIVE_RAIL_ADD_PROJECT_LABEL)
                    .expect("the native add-project button has a valid accessible label"),
            ),
        )
        .expect("the native add-project button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .disabled(disabled)
        .debug_selector(NATIVE_RAIL_ADD_PROJECT_SELECTOR)
        .on_activate(move |_, _, app| {
            let _ = application.update(app, |application, cx| {
                application.activate_add_project(cx);
            });
        })
    }

    fn message_retry_button(&mut self, cx: &mut Context<Self>) -> Button {
        let enabled = self.message_retry_is_admissible(cx);
        self.message_retry_focus_handle = self.message_retry_focus_handle.clone().tab_stop(enabled);
        let application = cx.entity().downgrade();
        Button::new(
            NATIVE_MESSAGE_RETRY_SELECTOR,
            self.message_retry_focus_handle.clone(),
            self.theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text(NATIVE_MESSAGE_RETRY_LABEL),
        )
        .expect("the native message retry button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .disabled(!enabled)
        .debug_selector(NATIVE_MESSAGE_RETRY_SELECTOR)
        .on_activate(move |_, _, app| {
            let _ = application.update(app, |application, cx| {
                application.activate_message_retry(cx);
            });
        })
    }

    fn message_status_panel(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let mut panel = message_status_panel(
            &self.theme,
            self.message_receipt.as_ref(),
            self.message_failure,
        )?;
        if self.message_retry.is_some()
            && self.command_submission_is_available()
            && !self.service_stopped
        {
            panel = panel.child(self.message_retry_button(cx));
        }
        Some(panel)
    }

    fn try_mount_pending_thread(&mut self, cx: &mut Context<Self>) {
        if self.shutdown_prepared {
            self.pending_thread = None;
            return;
        }
        let Some(pending_thread) = self.pending_thread.clone() else {
            return;
        };
        let switch_generation = self.thread_switch_flight.as_ref().and_then(|flight| {
            (matches!(&flight.phase, ThreadSwitchPhase::SubscribeAdmission { .. })
                && flight.target_thread.as_ref() == Some(&pending_thread))
            .then_some(flight.generation)
        });
        let same_mounted_thread = self.conversation_host.as_ref().is_some_and(|host| {
            self.selected_thread.as_ref() == Some(&pending_thread)
                && host.read(cx).controller_view().delivery.thread_id == pending_thread
        });
        if same_mounted_thread {
            self.pending_thread = None;
            if switch_generation.is_some() {
                self.submit_thread_switch_subscribe(cx);
                return;
            }
            let has_matching_snapshot = self
                .pending_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.thread_id() == &pending_thread);
            if has_matching_snapshot {
                if let Some(snapshot) = self.pending_snapshot.take()
                    && let Some(host) = self.conversation_host.clone()
                {
                    self.dispatch_snapshot(&host, snapshot, cx);
                }
            } else if self
                .conversation_host
                .as_ref()
                .is_some_and(|host| host.read(cx).controller_view().delivery.has_snapshot)
            {
                self.state = NativeViewState::Ready;
                self.sync_composer_availability(cx);
            }
            return;
        }
        if switch_generation.is_none() {
            self.retire_host(cx);
            if self.conversation_host.is_some() {
                return;
            }
        } else if self.conversation_host.is_some() {
            return;
        }
        let Some(thread_id) = self.pending_thread.take() else {
            return;
        };
        self.composer.update(cx, |composer, cx| {
            composer.switch_thread(
                thread_id.as_str().to_owned(),
                switch_generation.is_none(),
                cx,
            );
        });
        self.selected_thread = Some(thread_id.clone());
        if matches!(
            self.route(),
            NativeRoute::NewThread { .. } | NativeRoute::Thread { .. }
        ) {
            if let Some(project) = self.selected_project.clone() {
                self.navigate(
                    NativeRoute::Thread {
                        project,
                        thread: thread_id.clone(),
                    },
                    cx,
                );
            }
        }
        if switch_generation.is_none() {
            self.standalone_snapshot_thread = None;
        }
        self.sync_thread_picker_selected(cx);
        self.engine_settings.select_thread(Some(&thread_id));
        self.reset_composer_catalog(cx);
        self.request_engine_settings_for_selected(cx);
        let Ok(host) = ConversationHost::mount(thread_id.clone(), ThemeMode::Dark, &mut *cx) else {
            self.set_failure(invalid_service_failure(), cx);
            return;
        };
        let subscription = cx.observe(&host, |application, host, cx| {
            application.collect_host_effects(&host, cx);
            application.pump_host_boundary(&host, cx);
        });
        self.conversation_host = Some(host.clone());
        let images = self.message_images.clone();
        host.read(cx)
            .surface()
            .clone()
            .update(cx, |surface, cx| surface.set_message_images(images, cx));
        drop(self.conversation_host_subscription.replace(subscription));
        suppress_conversation_tab_stops(&host, cx);
        self.collect_host_effects(&host, cx);
        if switch_generation.is_some() {
            // A fresh subscription start owns the authoritative target
            // snapshot. Drop only this newly mounted host's initial snapshot
            // request; no separate RequestSnapshot is admitted for a switch.
            self.discard_initial_snapshot_request(&thread_id);
            self.submit_thread_switch_subscribe(cx);
        } else {
            self.pump_host_boundary(&host, cx);
            // Subscribe for durable PatchBatch delivery using current cursor when available.
            let after = host.read(cx).controller_view().delivery.cursor;
            if let Some(service) = self.service.clone() {
                match service.submit(NativeTransportCommand::Subscribe {
                    thread_id: thread_id.clone(),
                    after,
                }) {
                    Ok(()) => {}
                    Err(error) => self.set_failure(command_failure(error), cx),
                }
            }
        }
        if switch_generation.is_none()
            && self
                .pending_snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.thread_id() == &thread_id)
            && let Some(snapshot) = self.pending_snapshot.take()
        {
            self.dispatch_snapshot(&host, snapshot, cx);
        }
    }

    fn discard_initial_snapshot_request(&mut self, thread_id: &ThreadId) {
        self.conversation_effects.retain(|effect| {
            !matches!(
                effect,
                ConversationHostEffect::Controller(ConversationStateEffect::Delivery(
                    ConversationDeliveryEffect::RequestSnapshot { thread_id: requested, .. }
                )) if requested == thread_id
            )
        });
    }

    fn retire_host(&mut self, cx: &mut Context<Self>) {
        self.remember_switch_listing();
        if let Some(thread_id) = self.selected_thread.clone() {
            self.remember_switch_snapshot_thread(thread_id.clone());
            if let Some(service) = self.service.clone()
                && self.ordinary_unsubscribe_thread.as_ref() != Some(&thread_id)
            {
                match service.submit(NativeTransportCommand::Unsubscribe {
                    thread_id: thread_id.clone(),
                }) {
                    Ok(()) => self.ordinary_unsubscribe_thread = Some(thread_id),
                    Err(error) => self.set_failure(command_failure(error), cx),
                }
            }
        }
        self.sync_thread_picker_disabled(cx);
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        let Some(host) = self.conversation_host.clone() else {
            self.selected_thread = None;
            self.standalone_snapshot_thread = None;
            self.engine_settings.select_thread(None);
            self.reset_composer_catalog(cx);
            self.sync_thread_picker_selected(cx);
            self.sync_composer_availability(cx);
            cx.notify();
            return;
        };
        self.pump_host_boundary(&host, cx);
        if self.conversation_effects.is_empty() && host.read(cx).total_pending_effect_count() == 0 {
            Self::release_transient_scroll_custody(&host, cx);
            self.conversation_host = None;
            drop(self.conversation_host_subscription.take());
            self.selected_thread = None;
            self.ordinary_unsubscribe_thread = None;
            self.standalone_snapshot_thread = None;
            self.engine_settings.select_thread(None);
            self.reset_composer_catalog(cx);
            self.sync_thread_picker_selected(cx);
            self.sync_thread_picker_disabled(cx);
            self.sync_composer_availability(cx);
            cx.notify();
        }
    }

    fn release_transient_scroll_custody(host: &Entity<ConversationHost>, cx: &mut Context<Self>) {
        let surface = host.read(cx).surface().clone();
        surface.update(cx, |surface, _| {
            surface.release_transient_scroll_custody();
        });
    }

    fn collect_host_effects(&mut self, host: &Entity<ConversationHost>, cx: &mut Context<Self>) {
        for _ in 0..=CONVERSATION_HOST_MAX_EFFECTS {
            let (pending, total_pending) = {
                let host_ref = host.read(cx);
                (
                    host_ref.pending_effect_count(),
                    host_ref.total_pending_effect_count(),
                )
            };
            let available =
                CONVERSATION_HOST_MAX_EFFECTS.saturating_sub(self.conversation_effects.len());
            if pending > available || total_pending == 0 {
                break;
            }
            let effects = host.update(cx, |host, _| host.drain_effects());
            if effects.is_empty() {
                if host.read(cx).pending_effect_count() == 0 {
                    break;
                }
                continue;
            }
            self.conversation_effects.extend(effects);
        }
    }

    fn pump_host_boundary(&mut self, host: &Entity<ConversationHost>, cx: &mut Context<Self>) {
        let mut retried_surface = false;
        for _ in 0..=CONVERSATION_HOST_MAX_EFFECTS {
            self.collect_host_effects(host, cx);
            while let Some(effect) = self.conversation_effects.first().cloned() {
                match effect {
                    ConversationHostEffect::Controller(ConversationStateEffect::Delivery(
                        ConversationDeliveryEffect::RequestSnapshot { thread_id, .. },
                    )) => {
                        let Some(service) = self.service.clone() else {
                            self.set_failure(
                                ServiceFailure {
                                    stage: ServiceFailureStage::EventBridge,
                                    category: ServiceFailureCategory::ChannelClosed,
                                },
                                cx,
                            );
                            return;
                        };
                        match service.submit(NativeTransportCommand::RequestSnapshot(thread_id)) {
                            Ok(()) => {
                                self.conversation_effects.remove(0);
                            }
                            Err(CommandSendError::Busy) => {
                                self.set_failure(
                                    ServiceFailure {
                                        stage: ServiceFailureStage::EventBridge,
                                        category: ServiceFailureCategory::Backpressure,
                                    },
                                    cx,
                                );
                                return;
                            }
                            Err(CommandSendError::Stopped) => {
                                self.set_failure(
                                    ServiceFailure {
                                        stage: ServiceFailureStage::EventBridge,
                                        category: ServiceFailureCategory::ChannelClosed,
                                    },
                                    cx,
                                );
                                return;
                            }
                        }
                    }
                    ConversationHostEffect::Controller(
                        ConversationStateEffect::SceneInvalidated
                        | ConversationStateEffect::Delivery(ConversationDeliveryEffect::Invalidate),
                    ) => {
                        self.conversation_effects.remove(0);
                    }
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        effect,
                    )) => {
                        if !self.apply_viewport_effect(host, &effect, cx) {
                            return;
                        }
                        self.conversation_effects.remove(0);
                    }
                    ConversationHostEffect::ScrollIntent { target } => {
                        let surface = host.read(cx).surface().clone();
                        let accepted = surface.update(cx, |surface, surface_cx| {
                            surface.schedule_scroll_target(target, surface_cx)
                        });
                        if !accepted {
                            return;
                        }
                        self.conversation_effects.remove(0);
                    }
                    _ => {
                        self.set_failure(invalid_service_failure(), cx);
                        return;
                    }
                }
            }
            if !self.conversation_effects.is_empty() {
                return;
            }
            if host.read(cx).total_pending_effect_count() != 0 || retried_surface {
                return;
            }
            retried_surface = true;
            host.update(cx, ConversationHost::process_pending_actions);
        }
    }

    fn apply_viewport_effect(
        &mut self,
        host: &Entity<ConversationHost>,
        effect: &crate::conversation_view_machine::ViewportEffect,
        cx: &mut Context<Self>,
    ) -> bool {
        match effect {
            crate::conversation_view_machine::ViewportEffect::ShowJumpToLatest => {
                let surface = host.read(cx).surface().clone();
                surface.update(cx, |surface, surface_cx| {
                    surface.set_jump_to_latest_visible(true, surface_cx);
                });
            }
            crate::conversation_view_machine::ViewportEffect::HideJumpToLatest => {
                let surface = host.read(cx).surface().clone();
                surface.update(cx, |surface, surface_cx| {
                    surface.set_jump_to_latest_visible(false, surface_cx);
                });
            }
            crate::conversation_view_machine::ViewportEffect::RequestBottomScroll {
                generation,
            } => {
                let can_scroll = {
                    let view = host.read(cx).controller_view();
                    view.viewport_generation == *generation
                        && match &view.viewport_state {
                            ViewportState::Following => true,
                            ViewportState::Scrolling {
                                generation: active_generation,
                            } => *active_generation == *generation,
                            _ => false,
                        }
                };
                if can_scroll {
                    let surface = host.read(cx).surface().clone();
                    surface.update(cx, |surface, surface_cx| {
                        surface.scroll_to_bottom(surface_cx);
                    });
                }
            }
            crate::conversation_view_machine::ViewportEffect::None
            | crate::conversation_view_machine::ViewportEffect::InvalidateRender
            | crate::conversation_view_machine::ViewportEffect::CompletionRejected { .. } => {}
            crate::conversation_view_machine::ViewportEffect::RequestAnchorRestore { .. }
            | crate::conversation_view_machine::ViewportEffect::GenerationExhausted => {
                self.set_failure(invalid_service_failure(), cx);
                return false;
            }
        }
        true
    }

    fn set_failure(&mut self, failure: ServiceFailure, cx: &mut Context<Self>) {
        self.clear_message_retry();
        self.state = NativeViewState::Failure(failure);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn reset_model_selector_offline(&mut self, cx: &mut Context<Self>) {
        let catalog = NativeModelCatalog::offline()
            .expect("the bundled model catalog is validated at the native boundary");
        self.model_selector.update(cx, |selector, cx| {
            selector.set_snapshot(catalog, cx);
            selector.set_policy(None, cx);
            selector.set_status(NativeModelSelectorStatus::default(), cx);
        });
    }

    fn reset_composer_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog_controller.clear_scope();
        self.reset_model_selector_offline(cx);
    }

    fn discover_composer_catalog(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        cx: &mut Context<Self>,
    ) {
        let selection = match self.catalog_controller.select_scope(thread_id, profile_id) {
            Ok(selection) => selection,
            Err(CatalogScopeError::GenerationExhausted) => {
                self.model_selector.update(cx, |selector, cx| {
                    selector.set_status(
                        NativeModelSelectorStatus {
                            saving: false,
                            error: Some(
                                "Runtime model catalog loading is unavailable for this session."
                                    .to_owned(),
                            ),
                            authoritative: false,
                        },
                        cx,
                    );
                });
                return;
            }
        };
        if selection.changed() {
            self.reset_model_selector_offline(cx);
        }
        self.submit_composer_catalog_reads(selection.scope().clone(), cx);
    }

    fn submit_composer_catalog_reads(&mut self, scope: NativeCatalogScope, cx: &mut Context<Self>) {
        let service = self.service.clone();
        if self.catalog_controller.catalog_request_needed(&scope) {
            let command = NativeTransportCommand::ReadComposerCatalog {
                thread_id: scope.thread_id.clone(),
                profile_id: scope.profile_id.clone(),
                generation: scope.generation,
            };
            let outcome = service
                .as_ref()
                .map_or(Err(CommandSendError::Stopped), |service| {
                    service.submit(command)
                });
            match outcome {
                Ok(()) => {
                    if !self.catalog_controller.mark_catalog_admitted(&scope) {
                        self.catalog_controller
                            .on_catalog_admission_failed(&scope, invalid_service_failure());
                    }
                }
                Err(error) => {
                    self.catalog_controller
                        .on_catalog_admission_failed(&scope, command_failure(error));
                }
            }
        }
        if self.catalog_controller.favorites_request_needed(&scope) {
            let command = NativeTransportCommand::ReadModelFavorites {
                thread_id: scope.thread_id.clone(),
                profile_id: scope.profile_id.clone(),
                generation: scope.generation,
            };
            let outcome = service
                .as_ref()
                .map_or(Err(CommandSendError::Stopped), |service| {
                    service.submit(command)
                });
            match outcome {
                Ok(()) => {
                    if !self.catalog_controller.mark_favorites_admitted(&scope) {
                        self.catalog_controller
                            .on_favorites_admission_failed(&scope, invalid_service_failure());
                    }
                }
                Err(error) => {
                    self.catalog_controller
                        .on_favorites_admission_failed(&scope, command_failure(error));
                }
            }
        }
        self.sync_composer_catalog_status(cx);
    }

    /// Retries the current catalog read without minting a new scope.
    pub fn retry_composer_catalog(&mut self, cx: &mut Context<Self>) {
        if let Some(scope) = self.catalog_controller.retry_catalog() {
            self.submit_composer_catalog_reads(scope, cx);
        }
    }

    /// Returns whether the current runtime catalog exposes an explicit retry.
    #[must_use]
    pub fn composer_catalog_retry_available(&self) -> bool {
        self.catalog_controller.catalog_retry_available()
    }

    /// Returns the truthful lifecycle of the selected runtime catalog.
    #[must_use]
    pub fn composer_catalog_phase(&self) -> NativeCatalogPhase {
        self.catalog_controller.catalog_phase()
    }

    /// Retries the current durable favorites read without minting a new scope.
    pub fn retry_composer_favorites(&mut self, cx: &mut Context<Self>) {
        if let Some(scope) = self.catalog_controller.retry_favorites() {
            self.submit_composer_catalog_reads(scope, cx);
        }
    }

    /// Returns whether the current durable favorites read exposes an explicit
    /// retry.
    #[must_use]
    pub fn composer_favorites_retry_available(&self) -> bool {
        self.catalog_controller.favorites_retry_available()
    }

    /// Retries the exact favorite mutation retained after a failure.
    pub fn retry_composer_favorite(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = self.catalog_controller.retry_favorite() {
            self.submit_pending_model_favorite(pending, cx);
        }
    }

    /// Returns whether an exact favorite mutation may be retried.
    #[must_use]
    pub fn composer_favorite_retry_available(&self) -> bool {
        self.catalog_controller.retry_favorite().is_some()
    }

    fn discover_composer_catalog_for_settings(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        let profile = self.engine_settings.authoritative_config()
            .map(|config| config.selection().as_opencode2().profile_id().clone())
            .or_else(|| match self.engine_settings.registry_view() {
                crate::engine_settings::RegistryView::Present(profiles) if profiles.len() == 1 => profiles.into_iter().next(),
                _ => None,
            });
        let Some(profile_id) = profile else {
            self.reset_composer_catalog(cx);
            return;
        };
        self.discover_composer_catalog(thread_id, profile_id, cx);
    }

    fn sync_composer_catalog_status(&mut self, cx: &mut Context<Self>) {
        let error = if self.catalog_controller.catalog_phase() == NativeCatalogPhase::Failed {
            Some("Runtime model catalog is unavailable. Retry model loading.".to_owned())
        } else if self.catalog_controller.favorites_failure().is_some() {
            Some("Model favorites could not be synchronized. Retry the favorite action.".to_owned())
        } else {
            None
        };
        let saving = self.catalog_controller.catalog_loading()
            || self
                .catalog_controller
                .pending_favorite()
                .is_some_and(|pending| pending.admitted);
        let authoritative = self.catalog_controller.catalog_ready();
        self.model_selector.update(cx, |selector, cx| {
            selector.set_status(
                NativeModelSelectorStatus {
                    saving,
                    error,
                    authoritative,
                },
                cx,
            );
        });
    }

    fn handle_composer_catalog(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
        result: artisan_protocol::ComposerCatalogResult,
        cx: &mut Context<Self>,
    ) {
        let scope = NativeCatalogScope::new(thread_id.clone(), profile_id.clone(), generation);
        if !self.catalog_controller.catalog_response_current(&scope)
            || result.thread_id != thread_id
            || result.profile_id != profile_id
        {
            return;
        }
        let mut catalog = match result.snapshot.decoded() {
            Ok(catalog) => catalog,
            Err(_) => {
                self.catalog_controller.on_catalog_failed(
                    &scope,
                    ServiceFailure {
                        stage: ServiceFailureStage::Request,
                        category: ServiceFailureCategory::Integrity,
                    },
                );
                self.sync_composer_catalog_status(cx);
                return;
            }
        };
        if catalog
            .scope
            .as_ref()
            .is_none_or(|catalog_scope| catalog_scope.profile_id.as_str() != profile_id.as_str())
        {
            self.catalog_controller.on_catalog_failed(
                &scope,
                ServiceFailure {
                    stage: ServiceFailureStage::Request,
                    category: ServiceFailureCategory::Integrity,
                },
            );
            self.sync_composer_catalog_status(cx);
            return;
        }
        if !self.catalog_controller.on_catalog_loaded(&scope) {
            return;
        }
        if self.catalog_controller.favorite_revision().is_some() {
            catalog.favorite_ids = self.catalog_controller.favorite_ids().to_vec();
        }
        self.model_selector.update(cx, |selector, cx| {
            selector.set_snapshot(catalog, cx);
        });
        self.sync_composer_model_policy(cx);
        self.sync_composer_catalog_status(cx);
        cx.notify();
    }

    fn handle_composer_catalog_failed(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let scope = NativeCatalogScope::new(thread_id, profile_id, generation);
        if self.catalog_controller.on_catalog_failed(&scope, failure) {
            self.sync_composer_catalog_status(cx);
            cx.notify();
        }
    }

    fn handle_model_favorites(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
        result: artisan_protocol::ModelFavoritesSnapshot,
        cx: &mut Context<Self>,
    ) {
        let scope = NativeCatalogScope::new(thread_id, profile_id, generation);
        if !self.catalog_controller.favorites_response_current(&scope) {
            return;
        }
        let revision = result.revision.get();
        let model_ids = result
            .model_ids
            .iter()
            .map(|model_id| model_id.as_str().to_owned())
            .collect::<Vec<_>>();
        let applied = self
            .catalog_controller
            .on_favorites_loaded(&scope, revision, model_ids);
        if applied {
            self.apply_authoritative_favorites(cx);
        }
        self.sync_composer_catalog_status(cx);
        cx.notify();
    }

    fn handle_model_favorites_failed(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let scope = NativeCatalogScope::new(thread_id, profile_id, generation);
        if self.catalog_controller.on_favorites_failed(&scope, failure) {
            self.sync_composer_catalog_status(cx);
            cx.notify();
        }
    }

    fn apply_authoritative_favorites(&mut self, cx: &mut Context<Self>) {
        let favorite_ids = self.catalog_controller.favorite_ids().to_vec();
        let mut catalog = self.model_selector.read(cx).state().snapshot().clone();
        catalog.favorite_ids = favorite_ids;
        self.model_selector.update(cx, |selector, cx| {
            selector.set_snapshot(catalog, cx);
        });
        self.sync_composer_model_policy(cx);
    }

    fn handle_model_favorite_set(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        request_id: RequestId,
        receipt: artisan_protocol::SetModelFavoriteReceipt,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.catalog_controller.scope().cloned() else {
            return;
        };
        if scope.thread_id != thread_id
            || scope.profile_id != profile_id
            || receipt.request_id != request_id
        {
            return;
        }
        let model_id = receipt.model_id.clone();
        let favorite = receipt.favorite;
        let revision = receipt.snapshot.revision.get();
        let model_ids = receipt
            .snapshot
            .model_ids
            .iter()
            .map(|model_id| model_id.as_str().to_owned())
            .collect::<Vec<_>>();
        if self.catalog_controller.on_favorite_succeeded(
            &scope,
            &request_id,
            &model_id,
            favorite,
            revision,
            model_ids,
        ) {
            self.apply_authoritative_favorites(cx);
            self.sync_composer_catalog_status(cx);
            cx.notify();
        }
    }

    fn handle_model_favorite_failed(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        request_id: RequestId,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.catalog_controller.scope().cloned() else {
            return;
        };
        if scope.thread_id != thread_id || scope.profile_id != profile_id {
            return;
        }
        if self
            .catalog_controller
            .on_favorite_failed(&scope, &request_id, failure)
        {
            self.sync_composer_catalog_status(cx);
            cx.notify();
        }
    }

    fn request_engine_settings_for_selected(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        if self.engine_settings.needs_registry_load() {
            self.submit_registry_load();
        }
        if self.engine_settings.needs_settings_load()
            || self.engine_settings.pending_reload_thread().is_some()
        {
            self.submit_settings_load(thread_id);
        }
        cx.notify();
    }

    fn submit_registry_load(&mut self) {
        if !self.engine_settings.needs_registry_load() {
            return;
        }
        let Some(service) = self.service.clone() else {
            self.engine_settings
                .on_registry_load_admission_failed(ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                });
            return;
        };
        match service.submit(NativeTransportCommand::ListRegisteredProfiles) {
            Ok(()) => self.engine_settings.mark_registry_load_admitted(),
            Err(error) => self
                .engine_settings
                .on_registry_load_admission_failed(command_failure(error)),
        }
    }

    fn submit_settings_load(&mut self, thread_id: ThreadId) {
        let generation = match self.engine_settings.prepare_settings_load() {
            Ok(generation) => generation,
            Err(failure) => {
                self.engine_settings
                    .on_settings_load_admission_failed(thread_id, failure);
                return;
            }
        };
        let command = NativeTransportCommand::LoadThreadEngineSettings {
            thread_id: thread_id.clone(),
            generation,
        };
        let Some(service) = self.service.clone() else {
            self.engine_settings.on_settings_load_admission_failed(
                thread_id,
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                },
            );
            return;
        };
        match service.submit(command) {
            Ok(()) => {
                if !self
                    .engine_settings
                    .mark_settings_load_admitted(&thread_id, generation)
                {
                    self.engine_settings.on_settings_load_admission_failed(
                        thread_id,
                        ServiceFailure {
                            stage: ServiceFailureStage::Request,
                            category: ServiceFailureCategory::Integrity,
                        },
                    );
                }
            }
            Err(error) => self
                .engine_settings
                .on_settings_load_admission_failed(thread_id, command_failure(error)),
        }
    }

    fn handle_engine_settings(
        &mut self,
        generation: SettingsLoadGeneration,
        result: artisan_protocol::ThreadEngineSettingsResult,
        cx: &mut Context<Self>,
    ) {
        let accepted = self.engine_settings.active_settings_generation() == Some(generation)
            && self
                .selected_thread
                .as_ref()
                .is_some_and(|thread_id| result.thread_id() == thread_id);
        self.engine_settings.on_settings_loaded(generation, result);
        self.sync_composer_model_policy(cx);
        if accepted {
            self.discover_composer_catalog_for_settings(cx);
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }

    fn handle_registered_profiles(
        &mut self,
        result: artisan_protocol::RegisteredEngineProfilesResult,
        cx: &mut Context<Self>,
    ) {
        self.engine_settings.on_registry_loaded(result);
        self.discover_composer_catalog_for_settings(cx);
        cx.notify();
    }

    fn handle_registered_profiles_failed(
        &mut self,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        self.engine_settings.on_registry_failed(failure);
        cx.notify();
    }

    fn handle_engine_config_set(
        &mut self,
        result: &artisan_protocol::SetThreadEngineConfigResult,
        retained: artisan_domain::EngineRunConfig,
        cx: &mut Context<Self>,
    ) {
        let accepted = self.engine_settings.selected_thread() == Some(&result.thread_id)
            && self.engine_settings.pending_save_request_id() == Some(&result.request_id);
        self.engine_settings.on_save_succeeded(result, retained);
        self.sync_composer_model_policy(cx);
        if accepted {
            self.discover_composer_catalog_for_settings(cx);
        }
        cx.notify();
    }

    fn handle_engine_conflict(
        &mut self,
        thread_id: ThreadId,
        request_id: &artisan_domain::RequestId,
        cx: &mut Context<Self>,
    ) {
        let accepted = self.engine_settings.selected_thread() == Some(&thread_id)
            && self.engine_settings.pending_save_request_id() == Some(request_id);
        self.engine_settings.on_conflict(thread_id, request_id);
        if accepted && self.engine_settings.pending_reload_thread().is_some() {
            self.reset_composer_catalog(cx);
        }
        if self.engine_settings.pending_reload_thread().is_some() {
            self.request_engine_settings_for_selected(cx);
        } else {
            cx.notify();
        }
    }

    fn handle_engine_config_failed(
        &mut self,
        thread_id: &ThreadId,
        request_id: &artisan_domain::RequestId,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        self.engine_settings
            .on_save_failed(thread_id, request_id, failure);
        self.sync_composer_model_policy(cx);
        cx.notify();
    }

    fn handle_engine_settings_failed(
        &mut self,
        thread_id: ThreadId,
        generation: SettingsLoadGeneration,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let accepted = self.engine_settings.active_settings_generation() == Some(generation)
            && self.selected_thread.as_ref() == Some(&thread_id);
        self.engine_settings
            .on_settings_load_failed(thread_id, generation, failure);
        if accepted {
            self.reset_composer_catalog(cx);
        }
        cx.notify();
    }

    fn select_engine_profile(&mut self, profile_id: &EngineProfileId, cx: &mut Context<Self>) {
        if self.engine_settings.select_profile(profile_id) {
            cx.notify();
        }
    }

    fn copy_manual_configuration_template(cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(manual_configuration_template()));
        cx.notify();
    }

    fn paste_manual_configuration_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let document = cx.read_from_clipboard().and_then(|item| item.text());
        if let Some(document) = document {
            let _ = self.engine_settings.apply_manual_configuration(&document);
        } else {
            // Route an absent/non-text clipboard through the same bounded
            // parser path without retaining or displaying clipboard data.
            let _ = self.engine_settings.apply_manual_configuration("");
        }
        cx.notify();
    }

    fn handle_copy_manual_configuration(_: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        Self::copy_manual_configuration_template(cx);
    }

    fn handle_paste_manual_configuration(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_manual_configuration_from_clipboard(cx);
    }

    fn handle_select_engine_profile(
        &mut self,
        profile_id: &EngineProfileId,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_engine_profile(profile_id, cx);
    }

    fn handle_save_engine_settings(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_engine_settings(cx);
    }

    fn handle_cancel_engine_settings(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_engine_settings(cx);
    }

    /// Attempts to save the current draft when valid and visible.
    pub fn save_engine_settings(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        if !self.engine_settings.can_save() {
            return;
        }
        let request_id = match create_save_request_id() {
            Ok(id) => id,
            Err(failure) => {
                self.engine_settings.on_save_admission_failed(failure);
                self.sync_composer_model_policy(cx);
                cx.notify();
                return;
            }
        };
        let Some(command) = self.engine_settings.build_save_command(request_id) else {
            self.engine_settings
                .on_save_admission_failed(ServiceFailure {
                    stage: ServiceFailureStage::Request,
                    category: ServiceFailureCategory::InvalidConfiguration,
                });
            self.sync_composer_model_policy(cx);
            cx.notify();
            return;
        };
        let request_id = command.request_id().clone();
        let retained_config = command.config().clone();
        let Some(service) = self.service.clone() else {
            self.engine_settings
                .on_save_admission_failed(ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                });
            self.sync_composer_model_policy(cx);
            cx.notify();
            return;
        };
        match service.submit(NativeTransportCommand::SetThreadEngineConfig(Box::new(
            command,
        ))) {
            Ok(()) => {
                if !self
                    .engine_settings
                    .begin_saving(thread_id, request_id, retained_config)
                {
                    self.engine_settings
                        .on_save_admission_failed(ServiceFailure {
                            stage: ServiceFailureStage::Request,
                            category: ServiceFailureCategory::Integrity,
                        });
                }
            }
            Err(error) => self
                .engine_settings
                .on_save_admission_failed(command_failure(error)),
        }
        self.sync_composer_model_policy(cx);
        cx.notify();
    }

    /// Cancels local edits without emitting a save.
    pub fn cancel_engine_settings(&mut self, cx: &mut Context<Self>) {
        self.engine_settings.cancel();
        self.sync_composer_model_policy(cx);
        cx.notify();
    }

    /// Returns the engine-settings controller for inspection.
    #[must_use]
    pub fn engine_settings(&self) -> &EngineSettingsController {
        &self.engine_settings
    }

    /// Returns a mutable reference to the engine-settings controller.
    pub fn engine_settings_mut(&mut self) -> &mut EngineSettingsController {
        &mut self.engine_settings
    }
}

static SAVE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
static MESSAGE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn create_save_request_id() -> Result<artisan_domain::RequestId, ServiceFailure> {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ServiceFailure {
                stage: ServiceFailureStage::Request,
                category: ServiceFailureCategory::Integrity,
            })?
            .as_millis(),
    )
    .map_err(|_| ServiceFailure {
        stage: ServiceFailureStage::Request,
        category: ServiceFailureCategory::Integrity,
    })?;
    let counter = SAVE_COUNTER
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |current| current.checked_add(1),
        )
        .map_err(|_| ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::Integrity,
        })?;
    let value = format!("engine-save-{millis}-{counter}");
    artisan_domain::RequestId::parse(value).map_err(|_| ServiceFailure {
        stage: ServiceFailureStage::Request,
        category: ServiceFailureCategory::Integrity,
    })
}

fn create_message_request_id() -> Result<RequestId, ServiceFailure> {
    let process_id = u64::from(std::process::id());
    let millis = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| ServiceFailure {
                stage: ServiceFailureStage::Request,
                category: ServiceFailureCategory::Integrity,
            })?
            .as_millis(),
    )
    .map_err(|_| ServiceFailure {
        stage: ServiceFailureStage::Request,
        category: ServiceFailureCategory::Integrity,
    })?;
    let counter = MESSAGE_COUNTER
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |current| current.checked_add(1),
        )
        .map_err(|_| ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::Integrity,
        })?;
    let value = format!("native-message-{process_id}-{millis}-{counter}");
    RequestId::parse(value).map_err(|_| ServiceFailure {
        stage: ServiceFailureStage::Request,
        category: ServiceFailureCategory::Integrity,
    })
}

fn submission_blocked_failure(blocked: SubmissionBlocked) -> Option<ServiceFailure> {
    match blocked {
        SubmissionBlocked::InvalidBody(_) => Some(ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::InvalidConfiguration,
        }),
        SubmissionBlocked::IdentityExhausted => Some(ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::Integrity,
        }),
        SubmissionBlocked::InFlight
        | SubmissionBlocked::Disabled
        | SubmissionBlocked::DraftChanged => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum PickerRoute {
    Select(ProjectId),
    BeginProjectIntake,
}

fn project_options_from_listing(listing: &ProjectListing) -> Vec<ProjectOption> {
    listing
        .projects()
        .iter()
        .map(|project| ProjectOption {
            id: project.project_id.clone(),
            name: gpui::SharedString::from(project.display_name.as_str().to_owned()),
        })
        .collect()
}

fn empty_thread_listing() -> ThreadListing {
    ThreadListing::new(Vec::new()).expect("an empty thread listing is always valid")
}

fn ready_membership_is_valid(
    projects: &ProjectListing,
    project_id: &ProjectId,
    threads: &artisan_domain::ThreadListing,
    thread_id: &ThreadId,
) -> bool {
    projects
        .projects()
        .iter()
        .any(|project| &project.project_id == project_id)
        && threads
            .threads()
            .iter()
            .all(|thread| &thread.project_id == project_id)
        && threads
            .threads()
            .iter()
            .any(|thread| &thread.thread_id == thread_id)
}

fn intake_command(retryable: bool) -> NativeTransportCommand {
    if retryable {
        NativeTransportCommand::RetryProjectIntake
    } else {
        NativeTransportCommand::BeginProjectIntake
    }
}

fn picker_route(
    action: &ProjectPickerAction,
    projects: &[ProjectOption],
) -> Result<PickerRoute, ServiceFailure> {
    match action {
        ProjectPickerAction::Choose(project_id) => {
            if projects.iter().any(|project| &project.id == project_id) {
                Ok(PickerRoute::Select(project_id.clone()))
            } else {
                Err(invalid_service_failure())
            }
        }
        ProjectPickerAction::NewProject => Ok(PickerRoute::BeginProjectIntake),
    }
}

fn command_failure(error: CommandSendError) -> ServiceFailure {
    match error {
        CommandSendError::Busy => ServiceFailure {
            stage: ServiceFailureStage::EventBridge,
            category: ServiceFailureCategory::Backpressure,
        },
        CommandSendError::Stopped => ServiceFailure {
            stage: ServiceFailureStage::EventBridge,
            category: ServiceFailureCategory::ChannelClosed,
        },
    }
}

const fn invalid_service_failure() -> ServiceFailure {
    ServiceFailure {
        stage: ServiceFailureStage::Request,
        category: ServiceFailureCategory::Integrity,
    }
}

fn suppress_conversation_tab_stops(
    host: &Entity<ConversationHost>,
    cx: &Context<NativeApplication>,
) {
    let (transcript_focus, disclosure_focus) = {
        let host_ref = host.read(cx);
        let surface = host_ref.surface().read(cx);
        (
            surface.transcript_focus_handle().clone(),
            surface.disclosure_focus_handle().clone(),
        )
    };
    transcript_focus.tab_stop(false);
    disclosure_focus.tab_stop(false);
}

fn status_panel(theme: &ArtisanTheme, state: &NativeViewState) -> Div {
    let (heading, detail): (&'static str, String) = match state {
        NativeViewState::Loading => (
            "Loading Artisan data",
            "Connecting to the owned local Forge.".to_owned(),
        ),
        NativeViewState::EmptyProjects => (
            "No attached projects",
            "Attach a project to begin a conversation.".to_owned(),
        ),
        NativeViewState::LoadingThreads => (
            "Loading project threads",
            "Reading the selected project from Forge.".to_owned(),
        ),
        NativeViewState::EmptyThreads => (
            "No threads in this project",
            "Choose another project or attach a new one.".to_owned(),
        ),
        NativeViewState::Ready => (
            "Conversation unavailable",
            "No conversation host is mounted.".to_owned(),
        ),
        NativeViewState::Failure(failure) => (
            "Native connection unavailable",
            format!("Service state: {failure}"),
        ),
    };
    status_panel_with_text(theme, heading, detail)
}

fn message_status_panel(
    theme: &ArtisanTheme,
    receipt: Option<&QueueMessageReceipt>,
    failure: Option<NativeMessageFailure>,
) -> Option<Div> {
    let detail = message_status_detail(receipt, failure)?;
    let style = CardStyle::resolve(*theme);
    Some(
        compact_card(style)
            .w_full()
            // The retry action is appended by the owning method as a second
            // card child, so the content-band inset lives on the root: both
            // the detail line and the action share one audited 16 px inset
            // and the root gap spaces them.
            .px(style.content_horizontal_padding)
            .child(
                div()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(detail),
            ),
    )
}

fn message_status_detail(
    receipt: Option<&QueueMessageReceipt>,
    failure: Option<NativeMessageFailure>,
) -> Option<String> {
    Some(if let Some(receipt) = receipt {
        let disposition = match receipt.disposition {
            artisan_domain::ReceiptDisposition::Accepted => "accepted",
            artisan_domain::ReceiptDisposition::Duplicate => "duplicate",
        };
        format!(
            "Message {disposition}; Forge message id {}.",
            receipt.message_id.as_str()
        )
    } else {
        let failure = failure?;
        format!(
            "Send failed: {} ({}).",
            failure.failure.stage, failure.failure.category
        )
    })
}

impl Render for NativeApplication {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_composer_controls(cx);
        let sidebar = self.desktop_sidebar(cx).into_any_element();
        let body = self.desktop_route_body(cx);
        let identity = self.desktop_identity(cx).into_any_element();
        let search = self.command_menu.clone().into_any_element();
        let shell = desktop_shell(
            self.desktop_theme,
            self.sidebar_collapsed,
            identity,
            search,
            sidebar,
            body,
            window.scale_factor(),
            window.is_maximized(),
        );
        div()
            .id("artisan-desktop-application-root")
            .track_focus(&self.focus_handle)
            .key_context(NATIVE_KEY_CONTEXT)
            .on_click(cx.listener(Self::dismiss_command_menu))
            .on_action(|_: &NextTabStop, window, cx| window.focus_next(cx))
            .on_action(|_: &PreviousTabStop, window, cx| window.focus_prev(cx))
            .on_action(cx.listener(Self::activate_command_menu))
            .size_full()
            .debug_selector(|| NATIVE_ROOT_SELECTOR.to_string())
            .relative()
            .child(shell)
            .child(self.message_images.clone())
    }
}

impl NativeApplication {
    /// Renders the route content for every route, mounting each
    /// route-port screen on first entry (or when the route identity changes)
    /// and reusing it afterwards.
    fn route_surface(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.route().clone() {
            NativeRoute::Onboarding => {
                if self.onboarding_screen.is_none() {
                    let theme = self.theme.clone();
                    let entries = HarnessCatalog::new()
                        .cards()
                        .iter()
                        .map(|card| {
                            OnboardingHarnessEntry::new(
                                card.clone(),
                                HarnessSetupState::new(
                                    HarnessSetupAction::default(),
                                    false,
                                    false,
                                    "Unavailable",
                                    None,
                                    None,
                                ),
                            )
                        })
                        .collect();
                    self.onboarding_screen =
                        Some(cx.new(move |_| OnboardingScreen::new(theme, entries)));
                }
                self.onboarding_screen
                    .clone()
                    .expect("onboarding screen mounted")
                    .into_any_element()
            }
            NativeRoute::Thread { thread, .. } => {
                let key = Some((thread.clone(), self.conversation_host.is_some()));
                if self.thread_screen_key != key || self.thread_screen.is_none() {
                    let mounted = match self.conversation_host.clone() {
                        Some(host) => {
                            let composer = self.composer.clone();
                            let screen = cx.new(|screen_cx| {
                                ThreadScreen::new(host, composer, ThemeMode::Dark, screen_cx)
                            });
                            screen.update(cx, |screen, _| {
                                screen.set_gate(ThreadScreenGate::Open);
                            });
                            Some(screen)
                        }
                        None => ThreadScreen::mount(thread.clone(), ThemeMode::Dark, cx).ok(),
                    };
                    self.thread_screen = mounted;
                    self.thread_screen_key = key;
                }
                self.thread_screen
                    .clone()
                    .map(|screen| screen.into_any_element())
                    .unwrap_or_else(|| status_panel(&self.theme, &self.state).into_any_element())
            }
            NativeRoute::Editor {
                project, thread, ..
            } => {
                let key = Some((project.clone(), thread.clone(), None));
                if self.editor_screen_key != key || self.editor_screen.is_none() {
                    let display_name = self
                        .project_options
                        .iter()
                        .find(|option| option.id == project)
                        .map_or_else(|| "Workspace".to_owned(), |option| option.name.to_string());
                    let identity = EditorScreenIdentity::new(
                        project.clone(),
                        display_name,
                        thread,
                        None,
                        None,
                    );
                    let screen = EditorScreen::new(
                        identity,
                        Vec::new(),
                        EditorSurfaceState::NoFile { recent: Vec::new() },
                        EditorViewState::default(),
                        self.theme.clone(),
                    );
                    self.editor_screen = Some(cx.new(|_| screen));
                    self.editor_screen_key = key;
                }
                self.editor_screen
                    .clone()
                    .expect("editor screen mounted")
                    .into_any_element()
            }
            NativeRoute::Settings { section, engine } => {
                let key = Some((section, engine.clone()));
                if self.settings_screen_key != key || self.settings_screen.is_none() {
                    let screen = cx.new(|screen_cx| {
                        SettingsScreen::new(section, engine, ThemeMode::Dark, screen_cx)
                    });
                    self.settings_screen = Some(screen);
                    self.settings_screen_key = key;
                }
                self.settings_screen
                    .clone()
                    .expect("settings screen mounted")
                    .into_any_element()
            }
            NativeRoute::NewThread { .. } => self.new_thread_surface_section().into_any_element(),
        }
    }
}

fn intake_status_panel(theme: &ArtisanTheme, stage: NativeProjectIntakeStage) -> Div {
    let (heading, detail) = match stage {
        NativeProjectIntakeStage::PickingDirectory => (
            "Choose a project folder",
            "Waiting for the native folder chooser.".to_owned(),
        ),
        NativeProjectIntakeStage::AttachingProject => (
            "Attaching project",
            "Saving the selected project in Forge.".to_owned(),
        ),
        NativeProjectIntakeStage::RefreshingProjects => (
            "Refreshing projects",
            "Reading the authoritative project catalog.".to_owned(),
        ),
        NativeProjectIntakeStage::CreatingThread => (
            "Creating a new thread",
            "Saving the new thread in Forge.".to_owned(),
        ),
        NativeProjectIntakeStage::RefreshingThreads => (
            "Refreshing threads",
            "Reading the authoritative thread catalog.".to_owned(),
        ),
    };
    status_panel_with_text(theme, heading, detail)
}

fn intake_failure_panel(theme: &ArtisanTheme, retryable: bool) -> Div {
    let detail = if retryable {
        "The project intake could not finish. Choose the project control to retry.".to_owned()
    } else {
        "The project intake could not finish. Choose a new project to try again.".to_owned()
    };
    status_panel_with_text(theme, "Project intake unavailable", detail)
}

fn status_panel_with_text(theme: &ArtisanTheme, heading: &'static str, detail: String) -> Div {
    let style = CardStyle::resolve(*theme);
    compact_card(style)
        .w_full()
        .debug_selector(|| NATIVE_STATUS_SELECTOR.to_string())
        .child(
            compact_card_content(style).child(
                div()
                    .text_size(theme.typography.dialog_title_text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(heading),
            ),
        )
        .child(separator(
            theme.colors.border.to_paint(),
            SeparatorAxis::Horizontal,
        ))
        .child(
            compact_card_content(style).child(
                div()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(detail),
            ),
        )
}

fn engine_settings_failure_detail(controller: &EngineSettingsController) -> String {
    match controller.failure_operation() {
        Some(EngineSettingsFailureOperation::Registry) => controller.service_failure().map_or_else(
            || "The certified profile catalogue is unavailable.".to_owned(),
            |failure| format!("Certified profile catalogue failure: {failure}."),
        ),
        Some(EngineSettingsFailureOperation::SettingsRead) => {
            controller.service_failure().map_or_else(
                || "Authoritative thread settings could not be read.".to_owned(),
                |failure| format!("Authoritative settings read failure: {failure}."),
            )
        }
        Some(EngineSettingsFailureOperation::Save) => controller.service_failure().map_or_else(
            || "The complete engine configuration was not saved.".to_owned(),
            |failure| format!("Engine settings save failure: {failure}."),
        ),
        Some(EngineSettingsFailureOperation::Input) => controller.input_error().map_or_else(
            || "The manual configuration was rejected.".to_owned(),
            |error| {
                format!(
                    "Manual configuration rejected: field {} ({}).",
                    error.field(),
                    error.reason()
                )
            },
        ),
        None => "Engine settings could not be loaded.".to_owned(),
    }
}

fn engine_settings_status_detail(
    controller: &EngineSettingsController,
    selected_thread: Option<&ThreadId>,
) -> (&'static str, String) {
    match controller.status() {
        EngineSettingsStatus::Unselected => (
            "Engine settings — no thread selected",
            "Select a real thread to load engine settings.".to_owned(),
        ),
        EngineSettingsStatus::Loading => (
            "Engine settings — loading",
            selected_thread.map_or("Loading authoritative settings.".to_owned(), |id| {
                format!("Loading settings for thread {}.", id.as_str())
            }),
        ),
        EngineSettingsStatus::RegistryMissing => (
            "Engine settings — registry missing",
            "No engine registry found. No certified profile is available.".to_owned(),
        ),
        EngineSettingsStatus::RegistryPresentEmpty => (
            "Engine settings — registry present (empty)",
            "Registry exists but contains no certified profile IDs.".to_owned(),
        ),
        EngineSettingsStatus::Unconfigured => (
            "Engine settings — unconfigured",
            selected_thread.map_or("Thread has no engine configuration.".to_owned(), |id| {
                format!("Thread {} has no engine configuration.", id.as_str())
            }),
        ),
        EngineSettingsStatus::Ready => {
            let revision = controller
                .revision()
                .map_or("unknown revision".to_owned(), |revision| {
                    format!("rev {}", revision.get())
                });
            (
                "Engine settings — configured",
                format!(
                    "Thread {} is configured ({revision}).",
                    selected_thread.map_or("unknown", |id| id.as_str())
                ),
            )
        }
        EngineSettingsStatus::Dirty => (
            "Engine settings — dirty",
            "Local edits differ from authoritative. Save is required.".to_owned(),
        ),
        EngineSettingsStatus::Saving => (
            "Engine settings — saving",
            "Persisting complete EngineRunConfig.".to_owned(),
        ),
        EngineSettingsStatus::ConflictRefreshing => (
            "Engine settings — conflict, refreshing",
            controller.service_failure().map_or_else(
                || {
                    "Save conflicted. Reloading authoritative settings before edits can resume."
                        .to_owned()
                },
                |failure| {
                    format!("Save conflicted; authoritative refresh is still pending ({failure}).")
                },
            ),
        ),
        EngineSettingsStatus::Failure => (
            "Engine settings — unavailable",
            engine_settings_failure_detail(controller),
        ),
    }
}

fn certified_profiles_detail(registry_view: &RegistryView) -> String {
    match registry_view {
        RegistryView::Present(ids) if ids.is_empty() => {
            "Certified profiles: none (registry empty)".to_owned()
        }
        RegistryView::Present(ids) => {
            let list = ids
                .iter()
                .map(EngineProfileId::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            format!("Certified profiles: {list}")
        }
        RegistryView::Missing => "Certified profiles: registry missing".to_owned(),
        RegistryView::PresentEmpty => "Certified profiles: none".to_owned(),
        RegistryView::Loading => "Certified profiles: loading".to_owned(),
    }
}

fn certified_profile_choices(
    theme: &ArtisanTheme,
    controller: &EngineSettingsController,
    registry_view: &RegistryView,
    cx: &Context<NativeApplication>,
) -> Div {
    let draft = controller.draft();
    let mut choices = div().flex().flex_col().gap_1();
    if let RegistryView::Present(ids) = registry_view {
        for (index, profile_id) in ids.iter().enumerate() {
            let profile_id_for_click = profile_id.clone();
            let label = if draft.profile_id == profile_id.as_str() {
                format!("✓ certified profile: {}", profile_id.as_str())
            } else {
                format!("certified profile: {}", profile_id.as_str())
            };
            let selector = format!("{NATIVE_ENGINE_SETTINGS_SELECTOR}-profile-{index}");
            choices = choices.child(
                div()
                    .id((NATIVE_ENGINE_SETTINGS_SELECTOR, index))
                    .debug_selector(move || selector)
                    .on_click(cx.listener(move |application, event, window, cx| {
                        application.handle_select_engine_profile(
                            &profile_id_for_click,
                            event,
                            window,
                            cx,
                        );
                    }))
                    .p(px(4.0))
                    .text_sm()
                    .text_color(theme.colors.foreground.to_paint())
                    .child(label),
            );
        }
    }
    choices
}

fn engine_settings_value_details(controller: &EngineSettingsController) -> (String, String) {
    let draft = controller.draft();
    let manual_fields = format!(
        "Manual/unverified — model: '{}', route: '{}', variant: '{}', permission: '{}', agent: '{}', approval: '{}', fs: '{}', net: '{}', web-search: '{}'",
        draft.model_id,
        draft.route_id,
        if draft.variant_id.is_empty() {
            "(none)"
        } else {
            &draft.variant_id
        },
        draft.permission_id,
        draft.agent_id,
        draft.approval,
        draft.filesystem,
        draft.network,
        draft.web_search
    );
    let runtime_fields = format!(
        "Runtime — attempt: '{}', readiness: '{}', health: '{}', prompt: '{}', stream: '{}', close: '{}', json: '{}', sse_line: '{}', sse_event: '{}', readiness_line: '{}', headers: '{}', http_buf: '{}', stderr: '{}', observations: '{}' — all manual/unverified until A6",
        draft.attempt_budget,
        draft.readiness_budget,
        draft.health_budget,
        draft.prompt_budget,
        draft.stream_budget,
        draft.close_budget,
        draft.max_json_body_bytes,
        draft.max_sse_line_bytes,
        draft.max_sse_event_bytes,
        draft.max_readiness_line_bytes,
        draft.max_header_count,
        draft.max_http_buffer_bytes,
        draft.max_stderr_bytes,
        draft.observation_capacity
    );
    (manual_fields, runtime_fields)
}

fn engine_settings_action_controls(
    theme: &ArtisanTheme,
    controller: &EngineSettingsController,
    selected_thread: Option<&ThreadId>,
    cx: &Context<NativeApplication>,
) -> (
    Stateful<Div>,
    Stateful<Div>,
    Div,
    Stateful<Div>,
    Stateful<Div>,
) {
    let save_enabled = controller.can_save() && selected_thread.is_some();
    let cancel_enabled = controller.can_cancel();
    let action_detail = if save_enabled {
        "Save is enabled for this complete, valid, dirty configuration."
    } else {
        "Save is disabled until a complete valid dirty configuration is ready."
    };
    let copy_button = div()
        .id("artisan-native-engine-settings-copy-template")
        .debug_selector(|| format!("{NATIVE_ENGINE_SETTINGS_SELECTOR}-copy-template"))
        .on_click(cx.listener(|_, event, window, cx| {
            NativeApplication::handle_copy_manual_configuration(event, window, cx);
        }))
        .p(px(4.0))
        .text_sm()
        .text_color(theme.colors.foreground.to_paint())
        .child("Copy manual configuration template");
    let paste_button = div()
        .id("artisan-native-engine-settings-paste-configuration")
        .debug_selector(|| format!("{NATIVE_ENGINE_SETTINGS_SELECTOR}-paste-configuration"))
        .on_click(cx.listener(NativeApplication::handle_paste_manual_configuration))
        .p(px(4.0))
        .text_sm()
        .text_color(theme.colors.foreground.to_paint())
        .child("Paste complete manual configuration");
    let mut save_button = div()
        .id("artisan-native-engine-settings-save")
        .debug_selector(|| format!("{NATIVE_ENGINE_SETTINGS_SELECTOR}-save"))
        .p(px(4.0))
        .text_sm()
        .text_color(theme.colors.foreground.to_paint())
        .child("Save");
    if save_enabled {
        save_button =
            save_button.on_click(cx.listener(NativeApplication::handle_save_engine_settings));
    } else {
        save_button = save_button.opacity(0.5);
    }
    let mut cancel_button = div()
        .id("artisan-native-engine-settings-cancel")
        .debug_selector(|| format!("{NATIVE_ENGINE_SETTINGS_SELECTOR}-cancel"))
        .p(px(4.0))
        .text_sm()
        .text_color(theme.colors.foreground.to_paint())
        .child("Cancel edits");
    if cancel_enabled {
        cancel_button =
            cancel_button.on_click(cx.listener(NativeApplication::handle_cancel_engine_settings));
    } else {
        cancel_button = cancel_button.opacity(0.5);
    }
    let action_detail = div()
        .text_sm()
        .text_color(if save_enabled {
            theme.colors.foreground.to_paint()
        } else {
            theme.colors.muted_foreground.to_paint()
        })
        .child(action_detail);
    (
        copy_button,
        paste_button,
        action_detail,
        save_button,
        cancel_button,
    )
}

fn engine_settings_panel(
    theme: &ArtisanTheme,
    controller: &EngineSettingsController,
    selected_thread: Option<&ThreadId>,
    cx: &Context<NativeApplication>,
) -> Div {
    let registry_view = controller.registry_view();
    let (heading, detail) = engine_settings_status_detail(controller, selected_thread);
    let certified_profiles = certified_profiles_detail(&registry_view);
    let certified_profile_choices =
        certified_profile_choices(theme, controller, &registry_view, cx);
    let (manual_fields, runtime_fields) = engine_settings_value_details(controller);
    let thread_bound = selected_thread.map_or("No thread bound".to_owned(), |id| {
        format!("Bound to thread {}", id.as_str())
    });
    let (copy_button, paste_button, action_detail, save_button, cancel_button) =
        engine_settings_action_controls(theme, controller, selected_thread, cx);
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .p(px(16.0))
        .mt(px(12.0))
        .rounded(px(8.0))
        .bg(theme.sidebar.sidebar.to_paint())
        .text_color(theme.colors.foreground.to_paint())
        .debug_selector(|| NATIVE_ENGINE_SETTINGS_SELECTOR.to_string())
        .child(heading)
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(detail),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(thread_bound),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(certified_profiles),
        )
        .child(certified_profile_choices)
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child("Manual/unverified clipboard configuration; values remain uncertified until A6."),
        )
        .child(copy_button)
        .child(paste_button)
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(manual_fields),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(runtime_fields),
        )
        .child(action_detail)
        .child(save_button)
        .child(cancel_button)
}

fn bind_native_actions(cx: &mut App) {
    NativeComposer::bind_actions(cx);
    NativeCommandMenu::bind_actions(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-q", Quit, None),
        KeyBinding::new("cmd-k", OpenCommandMenu, Some(NATIVE_KEY_CONTEXT)),
        KeyBinding::new("ctrl-k", OpenCommandMenu, Some(NATIVE_KEY_CONTEXT)),
        KeyBinding::new("tab", NextTabStop, Some(NATIVE_KEY_CONTEXT)),
        KeyBinding::new("shift-tab", PreviousTabStop, Some(NATIVE_KEY_CONTEXT)),
    ]);
}

fn request_app_shutdown(
    cx: &mut App,
    service: Option<Arc<NativeTransportService>>,
    shutdown_started: &Arc<AtomicBool>,
) {
    if shutdown_started.swap(true, Ordering::AcqRel) {
        return;
    }
    let task = cx.spawn(async move |cx| {
        if let Some(service) = service {
            let timer_executor = cx.background_executor().clone();
            loop {
                if service.is_finished() {
                    let _ = service.join();
                    break;
                }
                let _ = service.request_shutdown();
                while matches!(service.try_recv(), Ok(Some(_))) {}
                timer_executor.timer(POLL_INTERVAL).await;
            }
        }
        let _ = cx.update(|cx| cx.quit());
    });
    task.detach();
}

fn prepare_application_shutdown(
    view: &Rc<RefCell<Option<Entity<NativeApplication>>>>,
    cx: &mut App,
) {
    let view = view.borrow().clone();
    if let Some(view) = view {
        view.update(cx, NativeApplication::prepare_shutdown);
    }
}

/// Logs which GPU renderer backs the opened window.
///
/// Compile-time half of the renderer guard: `Window::gpu_context` only
/// exists when the `gpui/wgpu-surfaces` feature rides the workspace
/// `gpui_platform/wgpu` chain, so dropping that feature fails the build here
/// instead of silently running the DirectX backend. The runtime half is the
/// `eprintln!` below plus gpui_wgpu's own `Selected GPU adapter` log line.
#[cfg(target_os = "windows")]
fn report_renderer(window: &mut Window) {
    let wgpu_active = window.gpu_context().is_some();
    let device = window
        .gpu_specs()
        .map(|specs| specs.device_name)
        .unwrap_or_else(|| String::from("<unknown>"));
    eprintln!("artisan editor renderer: wgpu active = {wgpu_active}, device = {device}");
}

/// Non-Windows builds have no wgpu-shipping contract to guard.
#[cfg(not(target_os = "windows"))]
fn report_renderer(_window: &mut Window) {}

/// Launches the real native application window.
#[must_use]
pub fn run() -> ExitCode {
    let service = NativeTransportService::spawn().ok().map(Arc::new);
    let shutdown_started = Arc::new(AtomicBool::new(false));
    let launched = Rc::new(Cell::new(false));
    let launch_flag = Rc::clone(&launched);
    let application_view = Rc::new(RefCell::new(None));

    gpui_platform::application()
        .with_assets(artisan_ui::asset_seam::CatalogAssetSource)
        .run(move |cx: &mut App| {
            // Register the vendored legacy typefaces before any window opens;
            // on failure keep running on system faces (typed, not swallowed).
            if let Err(error) = artisan_ui::fonts::register_bundled_fonts(cx) {
                eprintln!("bundled font registration failed, using system faces: {error}");
            }

            bind_native_actions(cx);

            let service_for_action = service.clone();
            let shutdown_for_action = Arc::clone(&shutdown_started);
            let view_for_action = Rc::clone(&application_view);
            cx.on_action(move |_: &Quit, cx| {
                prepare_application_shutdown(&view_for_action, cx);
                request_app_shutdown(cx, service_for_action.clone(), &shutdown_for_action);
            });

            let service_for_close = service.clone();
            let shutdown_for_close = Arc::clone(&shutdown_started);
            let view_for_close = Rc::clone(&application_view);
            cx.on_window_closed(move |cx, _window_id| {
                if cx.windows().is_empty() {
                    prepare_application_shutdown(&view_for_close, cx);
                    request_app_shutdown(cx, service_for_close.clone(), &shutdown_for_close);
                }
            })
            .detach();

            let bounds = Bounds::centered(None, size(px(SURFACE_WIDTH), px(SURFACE_HEIGHT)), cx);
            let service_for_view = service.clone();
            let view_for_registration = Rc::clone(&application_view);
            let opened = cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions {
                        title: Some(WINDOW_TITLE.into()),
                        // CE keeps native resizing; desktop_shell supplies caption hit areas.
                        appears_transparent: true,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                move |window, cx| {
                    let view =
                        cx.new(|view_cx| NativeApplication::new(service_for_view, window, view_cx));
                    view_for_registration.borrow_mut().replace(view.clone());
                    view.update(cx, NativeApplication::start_polling);
                    view
                },
            );

            if opened.is_ok() {
                launch_flag.set(true);
                if let Ok(handle) = &opened {
                    let _ = handle.update(cx, |_, window, _| report_renderer(window));
                }
                cx.activate(true);
            } else {
                request_app_shutdown(cx, service.clone(), &shutdown_started);
            }
        });

    if launched.get() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::NATIVE_STATUS_SELECTOR;
    use super::{
        NATIVE_MESSAGE_RETRY_LABEL, NATIVE_MESSAGE_RETRY_SELECTOR, NATIVE_RAIL_ADD_PROJECT_LABEL,
        NativeApplication, NativeMessageFailure, NativeMessageFlight, NativeProjectIntakeOperation,
        NativeProjectIntakeStage, NativeTestCommandSink, NativeTransportCommand,
        NativeTransportEvent, NativeViewState, PickerRoute, ServiceFailure, ServiceStopStatus,
        ThreadSwitchFlight, ThreadSwitchPhase, WINDOW_TITLE, create_message_request_id,
        intake_command, message_status_detail, picker_route, project_options_from_listing,
        ready_membership_is_valid,
    };
    use crate::composer::{ComposerState, DraftDisposition};
    use crate::desktop_shell::{
        DESKTOP_COMPOSER_SELECTOR, DESKTOP_EMPTY_SELECTOR, DESKTOP_HOME_SELECTOR,
        DESKTOP_OFFLINE_SELECTOR, DESKTOP_SIDEBAR_SELECTOR, DESKTOP_TITLEBAR_SELECTOR,
    };
    use crate::native_command_menu::COMMAND_MENU_DROPDOWN_SELECTOR;
    use crate::native_route::{NativeRoute, SettingsRoute};
    use crate::{
        conversation_delivery_machine::ConversationDeliveryEffect,
        conversation_host::{ConversationHost, ConversationHostEffect},
        conversation_scene::SceneId,
        conversation_state_machine::ConversationStateEffect,
        conversation_surface::{
            CONVERSATION_SURFACE_MAX_SCROLL_TARGETS, ConversationSurfaceTarget,
        },
        conversation_view_machine::{CompletionRejection, ViewportEffect, ViewportGeneration},
        project_picker::{ProjectOption, ProjectPickerAction},
    };
    use artisan_domain::{
        ConversationCursor, ConversationSnapshot, ConversationSubscriptionStart, DisplayName,
        ProjectId, ProjectListing, ProjectSummary, ReceiptDisposition, RequestId, RootPath,
        ThreadId, ThreadListing, ThreadSummary, ThreadTitle, UnixMillis,
    };
    use artisan_protocol::{
        ConversationSubscriptionStarted, ConversationSubscriptionStopped, QueueMessageReceipt,
    };
    use artisan_ui::button::{
        Button, ButtonContent, ButtonSize, ButtonStyle, ButtonVariant, FocusVisibility,
    };
    use artisan_ui::dropdown_menu::{DropdownMenuEntry, DropdownMenuItem, DropdownMenuState};
    use artisan_ui::motion::MotionPolicy;
    use artisan_ui::tabs::{TabSpec, Tabs};
    use artisan_ui::theme::{ArtisanTheme, ThemeMode};
    use gpui::{Context, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext};
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    fn project(id: &str, name: &str) -> ProjectSummary {
        ProjectSummary {
            project_id: ProjectId::parse(id).expect("project"),
            display_name: DisplayName::parse(name).expect("display name"),
            root_path: RootPath::parse(format!("/{id}")).expect("root"),
            attached_at: UnixMillis::EPOCH,
        }
    }

    fn thread(id: &str, project_id: &str, title: &str) -> ThreadSummary {
        ThreadSummary {
            thread_id: ThreadId::parse(id).expect("thread"),
            project_id: ProjectId::parse(project_id).expect("project"),
            title: ThreadTitle::parse(title).expect("title"),
            created_at: UnixMillis::EPOCH,
            updated_at: UnixMillis::EPOCH,
        }
    }

    fn request(id: &str) -> RequestId {
        RequestId::parse(id).expect("request")
    }

    fn snapshot_for(thread_id: &ThreadId, cursor: u64) -> ConversationSnapshot {
        ConversationSnapshot::new(
            thread_id.clone(),
            ConversationCursor::new(cursor),
            Vec::new(),
            Vec::new(),
            UnixMillis::EPOCH,
        )
        .expect("snapshot")
    }

    fn fresh_start_event(
        thread_id: &ThreadId,
        request_id: &str,
        cursor: u64,
    ) -> NativeTransportEvent {
        NativeTransportEvent::ConversationSubscriptionStarted {
            thread_id: thread_id.clone(),
            request_id: request(request_id),
            started: ConversationSubscriptionStarted::Fresh(ConversationSubscriptionStart::new(
                snapshot_for(thread_id, cursor),
            )),
        }
    }

    fn command_sink(
        outcomes: impl IntoIterator<Item = Result<(), super::CommandSendError>>,
    ) -> (
        NativeTestCommandSink,
        Rc<RefCell<Vec<super::NativeTransportCommand>>>,
    ) {
        let commands = Rc::new(RefCell::new(Vec::new()));
        let sink = NativeTestCommandSink {
            commands: commands.clone(),
            outcomes: Rc::new(RefCell::new(outcomes.into_iter().collect::<VecDeque<_>>())),
        };
        (sink, commands)
    }

    fn message_failure() -> ServiceFailure {
        ServiceFailure {
            stage: super::ServiceFailureStage::Request,
            category: super::ServiceFailureCategory::Peer,
        }
    }

    fn install_ready_message_surface(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
        thread_id: ThreadId,
        draft: &str,
        sink: NativeTestCommandSink,
    ) {
        let host = ConversationHost::mount(thread_id.clone(), ThemeMode::Dark, &mut *cx)
            .expect("message host");
        let project = application
            .selected_project
            .clone()
            .unwrap_or_else(|| ProjectId::parse("message-project").expect("project"));
        application.selected_project = Some(project.clone());
        application.route_history.navigate(NativeRoute::Thread {
            project,
            thread: thread_id.clone(),
        });
        application.selected_thread = Some(thread_id);
        application.state = NativeViewState::Ready;
        application.conversation_host = Some(host);
        application.test_command_sink = Some(sink);
        let draft = draft.to_owned();
        application.composer.update(cx, |composer, composer_cx| {
            composer.set_disabled(false, composer_cx);
            composer.set_draft(draft);
            composer_cx.notify();
        });
        application.sync_composer_availability(cx);
    }

    #[gpui::test]
    fn desktop_new_task_preserves_draft_and_blocks_repeat_creation(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                let old = ThreadId::parse("existing-task").expect("thread");
                install_ready_message_surface(
                    application,
                    cx,
                    old.clone(),
                    "keep this draft",
                    sink,
                );
                let project = application.selected_project.clone().expect("project");
                application.begin_new_task(cx);
                assert_eq!(
                    *commands.borrow(),
                    vec![NativeTransportCommand::CreateTask(project)]
                );
                assert_eq!(application.selected_thread.as_ref(), Some(&old));
                assert_eq!(application.composer.read(cx).draft(), "keep this draft");
                assert!(!application.message_submission_is_admissible(cx));
                application.begin_new_task(cx);
                application.begin_message_submission(cx);
                assert_eq!(commands.borrow().len(), 1);
                application.handle_intake_failed(
                    NativeProjectIntakeOperation::CreateThread,
                    message_failure(),
                    false,
                    cx,
                );
                assert_eq!(application.composer.read(cx).draft(), "keep this draft");
                assert!(!application.message_submission_is_admissible(cx));
            })
        });
    }

    #[gpui::test]
    fn desktop_route_mismatch_cannot_send_to_previous_task(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_ready_message_surface(
                    application,
                    cx,
                    ThreadId::parse("old-task").expect("thread"),
                    "keep",
                    sink,
                );
                assert!(application.message_submission_is_admissible(cx));
                application.navigate(
                    NativeRoute::NewThread {
                        project: application.selected_project.clone(),
                    },
                    cx,
                );
                application.begin_message_submission(cx);
                assert!(commands.borrow().is_empty());
                assert_eq!(application.composer.read(cx).draft(), "keep");
                application.navigate(
                    NativeRoute::Thread {
                        project: application.selected_project.clone().expect("project"),
                        thread: ThreadId::parse("different-task").expect("thread"),
                    },
                    cx,
                );
                application.begin_message_submission(cx);
                assert!(commands.borrow().is_empty());
            })
        });
    }

    #[gpui::test]
    fn desktop_busy_sidebar_navigation_keeps_visible_task_and_draft(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_ready_message_surface(
                    application,
                    cx,
                    ThreadId::parse("old-task").expect("thread"),
                    "keep",
                    sink,
                );
                application.thread_listing = Some(
                    ThreadListing::new(vec![thread("target-task", "message-project", "Target")])
                        .expect("listing"),
                );
                let old_route = application.route().clone();
                application.intake_stage = Some(NativeProjectIntakeStage::CreatingThread);
                application
                    .open_thread_from_sidebar(ThreadId::parse("target-task").expect("thread"), cx);
                assert_eq!(application.route(), &old_route);
                assert_eq!(application.composer.read(cx).draft(), "keep");
                assert!(commands.borrow().is_empty());
            })
        });
    }

    fn admit_message_flight(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
        request_id: &str,
    ) {
        let (body, token) = application.composer.update(cx, |composer, _| {
            composer
                .begin_payload_submission()
                .expect("message draft admits one flight")
        });
        let thread_id = application
            .selected_thread
            .clone()
            .expect("selected message thread");
        application.message_flight = Some(NativeMessageFlight {
            thread_id,
            request_id: request(request_id),
            payload: body,
            token,
        });
    }

    fn fail_active_message(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
    ) {
        let (thread_id, request_id) = {
            let flight = application.message_flight.as_ref().expect("message flight");
            (flight.thread_id.clone(), flight.request_id.clone())
        };
        application.handle_service_event(
            NativeTransportEvent::MessageFailed {
                thread_id,
                request_id,
                failure: message_failure(),
            },
            cx,
        );
    }

    fn complete_key_press(cx: &mut VisualTestContext, key: &'static str) {
        cx.simulate_event(KeyUpEvent {
            keystroke: Keystroke::parse(key).expect("known keyboard activation key"),
        });
    }

    #[test]
    fn real_project_summaries_become_identity_preserving_options() {
        let listing = ProjectListing::new(vec![
            project("forge-p1", "First"),
            project("forge-p2", "Second"),
        ])
        .expect("listing");
        let options = project_options_from_listing(&listing);
        assert_eq!(options.len(), 2);
        assert_eq!(options[0].id.as_str(), "forge-p1");
        assert_eq!(options[0].name.as_ref(), "First");
        assert_eq!(options[1].id.as_str(), "forge-p2");
    }

    #[test]
    fn picker_choose_routes_the_real_project_id_and_new_begins_intake() {
        let first = ProjectOption {
            id: ProjectId::parse("forge-p1").expect("project"),
            name: "First".into(),
        };
        let options = vec![first.clone()];
        assert_eq!(
            picker_route(&ProjectPickerAction::Choose(first.id.clone()), &options),
            Ok(PickerRoute::Select(first.id))
        );
        assert_eq!(
            picker_route(&ProjectPickerAction::NewProject, &options),
            Ok(PickerRoute::BeginProjectIntake)
        );
    }

    #[test]
    fn intake_actions_use_begin_then_the_single_retained_retry_command() {
        let options = vec![ProjectOption {
            id: ProjectId::parse("forge-p1").expect("project"),
            name: "First".into(),
        }];
        assert_eq!(
            picker_route(&ProjectPickerAction::NewProject, &options),
            Ok(PickerRoute::BeginProjectIntake)
        );
        assert_eq!(
            intake_command(false),
            crate::native_transport_service::NativeTransportCommand::BeginProjectIntake
        );
        assert_eq!(
            intake_command(true),
            crate::native_transport_service::NativeTransportCommand::RetryProjectIntake
        );
    }

    #[test]
    fn intake_bridge_refusals_stay_typed_and_redacted() {
        let busy = super::command_failure(super::CommandSendError::Busy);
        let stopped = super::command_failure(super::CommandSendError::Stopped);
        assert_eq!(busy.category, super::ServiceFailureCategory::Backpressure);
        assert_eq!(
            stopped.category,
            super::ServiceFailureCategory::ChannelClosed
        );
        assert!(!busy.to_string().contains("127.0.0.1"));
        assert!(!stopped.to_string().contains("directory"));
    }

    #[gpui::test]
    fn native_rail_add_project_has_stable_metadata_and_admission_policy(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, _) = command_sink([Ok(())]);

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                let disabled_button = application.add_project_button(application_cx);
                assert_eq!(
                    disabled_button.accessible_label(),
                    NATIVE_RAIL_ADD_PROJECT_LABEL
                );
                assert_eq!(application.add_project_focus_handle.tab_index, 0);
                assert!(!application.add_project_action_is_admissible());

                application.test_command_sink = Some(sink);
                let enabled_button = application.add_project_button(application_cx);
                assert_eq!(
                    enabled_button.accessible_label(),
                    NATIVE_RAIL_ADD_PROJECT_LABEL
                );
                assert_eq!(
                    enabled_button.visual_style(),
                    ButtonStyle::resolve(
                        application.theme,
                        ButtonVariant::Ghost,
                        ButtonSize::IconSmall,
                        MotionPolicy::Reduced,
                    )
                );
                assert_eq!(application.add_project_focus_handle.tab_index, 0);
                assert!(application.add_project_focus_handle.tab_stop);
                assert!(application.add_project_action_is_admissible());
                application_cx.notify();
            });
        });
        cx.run_until_parked();

        // The desktop workspace owns the active frame; the shared button
        // metadata and admission policy remain the contract for the sidebar.
    }

    #[gpui::test]
    fn navigation_changes_the_mounted_route_identity(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));

        // Fresh windows mount the default route.
        assert!(cx.debug_bounds("route-new-thread").is_some());

        // Navigating swaps the mounted route identity.
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::Appearance,
                        engine: None,
                    },
                    application_cx,
                );
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("route-settings-appearance").is_some());

        // Going back restores the default route identity.
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                assert!(application.go_back(application_cx));
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("route-new-thread").is_some());
    }

    #[gpui::test]
    fn ready_without_host_mounts_surface_on_new_thread_route(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.run_until_parked();
        assert!(cx.debug_bounds(DESKTOP_OFFLINE_SELECTOR).is_none());
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.state = NativeViewState::Ready;
                application_cx.notify();
            });
        });
        cx.run_until_parked();

        // Default route is NewThread: the concise home and actual composer
        // mount inside the desktop workspace, not the legacy activity recipe.
        assert!(cx.debug_bounds(DESKTOP_HOME_SELECTOR).is_some());
        assert!(cx.debug_bounds(DESKTOP_COMPOSER_SELECTOR).is_some());
        assert!(cx.debug_bounds(DESKTOP_SIDEBAR_SELECTOR).is_some());
        assert!(cx.debug_bounds(DESKTOP_TITLEBAR_SELECTOR).is_some());
        assert!(cx.debug_bounds("artisan-workspace-tabs-list").is_some());
        assert!(
            cx.debug_bounds(NATIVE_STATUS_SELECTOR).is_none(),
            "the Ready stub must not mount beside the surface"
        );

        // Other routes keep the status card.
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::Models,
                        engine: None,
                    },
                    application_cx,
                );
            });
        });
        cx.run_until_parked();
        // NOTE: `debug_bounds` keeps stale entries for selectors that have
        // left the tree, so absence of the surface is not assertable here;
        // the settings screen mounting (and the status card staying gone)
        // is the observable navigation outcome.
        assert!(cx.debug_bounds("route-settings-models").is_some());
        assert!(cx.debug_bounds(NATIVE_STATUS_SELECTOR).is_none());
    }

    #[gpui::test]
    fn wordmark_returns_to_start(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.update(|_, app| {
            view.update(app, |view, cx| {
                view.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::Models,
                        engine: None,
                    },
                    cx,
                );
            });
        });
        cx.run_until_parked();
        let brand = cx.debug_bounds("artisan-brand-home").expect("wordmark");
        cx.simulate_click(brand.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.update(|_, app| matches!(
            view.read(app).route(),
            NativeRoute::NewThread { project: None }
        )));
    }

    #[gpui::test]
    fn workspace_tabs_switch_by_pointer_and_keyboard(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.run_until_parked();
        let editor = cx
            .debug_bounds("artisan-workspace-tabs-trigger-editor")
            .expect("Editor tab");
        cx.simulate_click(editor.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).sidebar_editor));
        assert!(cx.debug_bounds("artisan-editor-empty").is_some());
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.sidebar_tabs_focus, cx));
        });
        cx.simulate_keystrokes("left");
        cx.run_until_parked();
        assert!(!cx.update(|_, app| view.read(app).sidebar_editor));
        assert!(cx.debug_bounds(DESKTOP_COMPOSER_SELECTOR).is_some());
    }

    #[gpui::test]
    fn profile_menu_keyboard_opens_settings_and_closes(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, app| view.read(app).profile_menu.is_open())
            .then_some(())
            .expect("menu opens");
        assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let trigger = cx
            .debug_bounds("artisan-desktop-profile-trigger")
            .expect("profile trigger");
        cx.simulate_click(trigger.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        cx.simulate_click(trigger.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        assert!(!cx.update(|_, app| view.read(app).profile_menu.is_open()));
        cx.simulate_keystrokes("enter home enter");
        cx.run_until_parked();
        cx.update(|_, app| {
            assert!(!view.read(app).profile_menu.is_open());
            assert!(matches!(
                view.read(app).route(),
                NativeRoute::Settings {
                    section: SettingsRoute::Models,
                    ..
                }
            ));
        });
    }

    #[gpui::test]
    fn command_shortcut_opens_shared_titlebar_search_and_escape_restores_root(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|_, app| super::bind_native_actions(app));
        cx.run_until_parked();
        let input = cx
            .debug_bounds(crate::native_command_menu::COMMAND_MENU_INPUT_SELECTOR)
            .expect("search input");
        cx.simulate_click(input.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|_, app| assert!(view.read(app).command_menu.read(app).state().is_open()));
        cx.simulate_keystrokes("escape");

        cx.simulate_keystrokes("ctrl-k");
        cx.run_until_parked();
        cx.update(|window, app| {
            let application = view.read(app);
            let menu = application.command_menu.read(app);
            assert!(menu.state().is_open());
            assert!(menu.input_focus().is_focused(window));
        });
        assert!(cx.debug_bounds(COMMAND_MENU_DROPDOWN_SELECTOR).is_some());
        assert!(
            cx.debug_bounds("artisan-native-command-menu-scrim")
                .is_none()
        );

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, app| {
            let application = view.read(app);
            assert!(!application.command_menu.read(app).state().is_open());
            assert!(application.focus_handle.is_focused(window));
        });
    }

    #[gpui::test]
    fn command_activation_routes_settings_through_the_application(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|_, app| super::bind_native_actions(app));
        cx.simulate_keystrokes("ctrl-k");
        cx.run_until_parked();
        cx.simulate_keystrokes("down");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, _| {
                assert_eq!(
                    application.route(),
                    &NativeRoute::Settings {
                        section: SettingsRoute::Models,
                        engine: None,
                    }
                );
            });
        });
    }

    #[gpui::test]
    fn admitted_rail_activation_submits_once_and_retains_restore_state(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Ok(())]);

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.state = NativeViewState::EmptyProjects;
                application.test_command_sink = Some(sink);
                application_cx.notify();
            });
        });
        cx.run_until_parked();

        // The rail button is retired pending the legacy rail re-homing;
        // drive the same activation handler it invoked.
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_add_project(application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, _| {
                assert!(matches!(
                    commands.borrow().as_slice(),
                    [NativeTransportCommand::BeginProjectIntake]
                ));
                assert!(matches!(
                    application.intake_stage,
                    Some(NativeProjectIntakeStage::PickingDirectory)
                ));
                assert!(matches!(
                    application.intake_restore_state.as_ref(),
                    Some(NativeViewState::EmptyProjects)
                ));
                assert!(!application.add_project_action_is_admissible());
            });
        });

        // Activation is now inadmissible: driving it again must not queue a
        // second intake command.
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_add_project(application_cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(commands.borrow().len(), 1);
    }

    #[gpui::test]
    fn rail_busy_and_stopped_admission_preserve_typed_failures(cx: &mut TestAppContext) {
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([
            Err(super::CommandSendError::Busy),
            Err(super::CommandSendError::Stopped),
        ]);

        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.state = NativeViewState::EmptyProjects;
                application.test_command_sink = Some(sink);
                application.activate_add_project(application_cx);
                assert!(matches!(
                    commands.borrow().as_slice(),
                    [NativeTransportCommand::BeginProjectIntake]
                ));
                assert!(application.intake_stage.is_none());
                assert!(matches!(
                    &application.state,
                    NativeViewState::Failure(failure)
                        if failure.stage == super::ServiceFailureStage::EventBridge
                            && failure.category == super::ServiceFailureCategory::Backpressure
                ));

                application.activate_add_project(application_cx);
                assert!(matches!(
                    commands.borrow().as_slice(),
                    [
                        NativeTransportCommand::BeginProjectIntake,
                        NativeTransportCommand::BeginProjectIntake
                    ]
                ));
                assert!(application.intake_stage.is_none());
                assert!(matches!(
                    &application.state,
                    NativeViewState::Failure(failure)
                        if failure.stage == super::ServiceFailureStage::EventBridge
                            && failure.category == super::ServiceFailureCategory::ChannelClosed
                ));
            });
        });
    }

    #[gpui::test]
    fn every_project_action_fence_disables_the_native_rail_action(cx: &mut TestAppContext) {
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Ok(())]);

        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.test_command_sink = Some(sink);
                assert!(application.add_project_action_is_admissible());

                application.shutdown_prepared = true;
                assert!(!application.add_project_action_is_admissible());
                application.shutdown_prepared = false;

                application.service_stopped = true;
                assert!(!application.add_project_action_is_admissible());
                application.service_stopped = false;

                application.intake_stage = Some(NativeProjectIntakeStage::PickingDirectory);
                assert!(!application.add_project_action_is_admissible());
                application.intake_stage = None;

                application.thread_switch_flight = Some(ThreadSwitchFlight {
                    source_thread: ThreadId::parse("rail-source").expect("thread"),
                    target_thread: Some(ThreadId::parse("rail-target").expect("thread")),
                    generation: 1,
                    phase: ThreadSwitchPhase::UnsubscribeAdmission {
                        retry_pending: false,
                        retry_used: false,
                    },
                });
                assert!(!application.add_project_action_is_admissible());
                application.thread_switch_flight = None;

                application.ordinary_unsubscribe_thread =
                    Some(ThreadId::parse("rail-unsubscribe").expect("thread"));
                assert!(!application.add_project_action_is_admissible());
                application.ordinary_unsubscribe_thread = None;

                assert!(application.add_project_action_is_admissible());
                assert!(commands.borrow().is_empty());
                application_cx.notify();
            });
        });
    }

    #[test]
    fn ready_membership_requires_the_exact_project_and_thread_rows() {
        let projects = ProjectListing::new(vec![
            project("forge-p1", "First"),
            project("forge-p2", "Second"),
        ])
        .expect("projects");
        let threads = ThreadListing::new(vec![
            thread("forge-t1", "forge-p2", "Existing"),
            thread("forge-t2", "forge-p2", "New thread"),
        ])
        .expect("threads");
        assert!(ready_membership_is_valid(
            &projects,
            &ProjectId::parse("forge-p2").expect("project"),
            &threads,
            &ThreadId::parse("forge-t2").expect("thread")
        ));
        assert!(!ready_membership_is_valid(
            &projects,
            &ProjectId::parse("missing-project").expect("project"),
            &threads,
            &ThreadId::parse("forge-t2").expect("thread")
        ));
        let cross_project_threads =
            ThreadListing::new(vec![thread("forge-t2", "forge-p1", "New thread")])
                .expect("threads");
        assert!(!ready_membership_is_valid(
            &projects,
            &ProjectId::parse("forge-p2").expect("project"),
            &cross_project_threads,
            &ThreadId::parse("forge-t2").expect("thread")
        ));
    }

    #[gpui::test]
    fn picker_is_disabled_for_every_intake_progress_stage(cx: &mut TestAppContext) {
        let project_id = ProjectId::parse("forge-p1").expect("project");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.install_picker(
                    vec![ProjectOption {
                        id: project_id.clone(),
                        name: "First".into(),
                    }],
                    Some(project_id),
                    application_cx,
                );
                for stage in [
                    NativeProjectIntakeStage::PickingDirectory,
                    NativeProjectIntakeStage::AttachingProject,
                    NativeProjectIntakeStage::RefreshingProjects,
                    NativeProjectIntakeStage::CreatingThread,
                    NativeProjectIntakeStage::RefreshingThreads,
                ] {
                    application.handle_intake_progress(stage, application_cx);
                    let picker = application.picker.clone().expect("picker");
                    assert!(picker.read(application_cx).state().is_disabled());
                }
            });
        });
    }

    #[gpui::test]
    fn cancellation_restores_the_prior_catalog_and_host_and_clears_picker_action(
        cx: &mut TestAppContext,
    ) {
        let project_id = ProjectId::parse("forge-p1").expect("project");
        let thread_id = ThreadId::parse("forge-t1").expect("thread");
        let options = vec![ProjectOption {
            id: project_id.clone(),
            name: "First".into(),
        }];
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.project_options = options.clone();
                application.selected_project = Some(project_id.clone());
                application.pending_thread = Some(thread_id.clone());
                application.try_mount_pending_thread(application_cx);
                let host_before = application.conversation_host.clone().expect("host");
                application.state = NativeViewState::Ready;
                application.install_picker(
                    options.clone(),
                    Some(project_id.clone()),
                    application_cx,
                );
                application.last_picker_action = Some(ProjectPickerAction::NewProject);
                application.handle_intake_progress(
                    NativeProjectIntakeStage::PickingDirectory,
                    application_cx,
                );
                assert!(
                    application
                        .picker
                        .as_ref()
                        .expect("picker")
                        .read(application_cx)
                        .state()
                        .is_disabled()
                );

                application.handle_intake_cancelled(application_cx);

                assert!(matches!(&application.state, NativeViewState::Ready));
                assert_eq!(application.project_options, options);
                assert_eq!(application.selected_project.as_ref(), Some(&project_id));
                assert_eq!(application.selected_thread.as_ref(), Some(&thread_id));
                assert_eq!(application.conversation_host.as_ref(), Some(&host_before));
                let picker = application
                    .picker
                    .as_ref()
                    .expect("picker")
                    .read(application_cx);
                assert!(!picker.state().is_disabled());
                assert_eq!(picker.last_action(), None);
                assert_eq!(application.intake_stage, None);
                assert_eq!(application.intake_failure_operation, None);
            });
        });
    }

    #[gpui::test]
    fn retryable_intake_failure_keeps_a_picker_for_the_retry_command(cx: &mut TestAppContext) {
        let project_id = ProjectId::parse("forge-p1").expect("project");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.install_picker(
                    vec![ProjectOption {
                        id: project_id.clone(),
                        name: "First".into(),
                    }],
                    Some(project_id),
                    application_cx,
                );
                application.handle_intake_progress(
                    NativeProjectIntakeStage::CreatingThread,
                    application_cx,
                );
                application.handle_intake_failed(
                    NativeProjectIntakeOperation::CreateThread,
                    ServiceFailure {
                        stage: super::ServiceFailureStage::Request,
                        category: super::ServiceFailureCategory::Peer,
                    },
                    true,
                    application_cx,
                );
                assert!(application.intake_retry_available);
                assert_eq!(
                    intake_command(true),
                    super::NativeTransportCommand::RetryProjectIntake
                );
                assert!(
                    !application
                        .picker
                        .as_ref()
                        .expect("picker")
                        .read(application_cx)
                        .state()
                        .is_disabled()
                );
                assert_eq!(
                    application
                        .picker
                        .as_ref()
                        .expect("picker")
                        .read(application_cx)
                        .last_action(),
                    None
                );
                assert!(matches!(&application.state, NativeViewState::Failure(_)));
            });
        });
    }

    #[gpui::test]
    fn ready_mounts_the_exact_returned_project_and_thread_and_requests_its_snapshot(
        cx: &mut TestAppContext,
    ) {
        let projects = ProjectListing::new(vec![
            project("forge-p1", "First"),
            project("forge-p2", "Second"),
        ])
        .expect("projects");
        let threads = ThreadListing::new(vec![
            thread("forge-t1", "forge-p2", "Existing"),
            thread("forge-t2", "forge-p2", "New thread"),
        ])
        .expect("threads");
        let project_id = ProjectId::parse("forge-p2").expect("project");
        let thread_id = ThreadId::parse("forge-t2").expect("thread");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));

        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.handle_intake_ready(
                    &projects,
                    project_id.clone(),
                    &threads,
                    thread_id.clone(),
                    application_cx,
                );
                assert_eq!(application.selected_project.as_ref(), Some(&project_id));
                assert_eq!(application.selected_thread.as_ref(), Some(&thread_id));
                assert_eq!(application.project_options[0].id.as_str(), "forge-p1");
                assert_eq!(application.project_options[1].id.as_str(), "forge-p2");
                let host = application.conversation_host.as_ref().expect("host");
                assert_eq!(
                    host.read(application_cx)
                        .controller_view()
                        .delivery
                        .thread_id,
                    thread_id
                );
                assert!(matches!(
                    application.conversation_effects.as_slice(),
                    [ConversationHostEffect::Controller(
                        ConversationStateEffect::Delivery(
                            ConversationDeliveryEffect::RequestSnapshot {
                                thread_id: requested,
                                ..
                            }
                        )
                    )] if requested == &thread_id
                ));
            });
        });
    }

    #[gpui::test]
    fn mismatched_ready_does_not_replace_the_real_host_or_add_rows(cx: &mut TestAppContext) {
        let old_project_id = ProjectId::parse("forge-p1").expect("project");
        let old_thread_id = ThreadId::parse("forge-t1").expect("thread");
        let options = vec![ProjectOption {
            id: old_project_id.clone(),
            name: "First".into(),
        }];
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.project_options = options.clone();
                application.selected_project = Some(old_project_id.clone());
                application.pending_thread = Some(old_thread_id.clone());
                application.try_mount_pending_thread(application_cx);
                let host_before = application.conversation_host.clone().expect("host");
                application.install_picker(
                    options.clone(),
                    Some(old_project_id.clone()),
                    application_cx,
                );
                application.last_picker_action = Some(ProjectPickerAction::NewProject);
                let mismatched_projects =
                    ProjectListing::new(vec![project("forge-p1", "First")]).expect("projects");
                let mismatched_threads =
                    ThreadListing::new(vec![thread("forge-t2", "forge-p2", "New thread")])
                        .expect("threads");
                application.handle_intake_ready(
                    &mismatched_projects,
                    ProjectId::parse("forge-p2").expect("project"),
                    &mismatched_threads,
                    ThreadId::parse("forge-t2").expect("thread"),
                    application_cx,
                );
                assert_eq!(application.project_options, options);
                assert_eq!(application.conversation_host.as_ref(), Some(&host_before));
                assert_eq!(
                    application
                        .picker
                        .as_ref()
                        .expect("picker")
                        .read(application_cx)
                        .state()
                        .projects(),
                    options.as_slice()
                );
                assert_eq!(
                    application
                        .picker
                        .as_ref()
                        .expect("picker")
                        .read(application_cx)
                        .last_action(),
                    None
                );
                assert!(matches!(&application.state, NativeViewState::Failure(_)));
            });
        });
    }

    #[gpui::test]
    fn real_thread_host_mount_retains_exact_initial_snapshot_request(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("forge-thread").expect("thread");
        let (host, _) = cx.add_window_view(|_, host_cx| {
            ConversationHost::new(thread_id.clone(), ThemeMode::Dark, host_cx).expect("host")
        });
        let effects = cx.update(|app| host.update(app, |host, _| host.drain_effects()));
        assert!(matches!(
            effects.as_slice(),
            [ConversationHostEffect::Controller(
                ConversationStateEffect::Delivery(
                    ConversationDeliveryEffect::RequestSnapshot {
                        thread_id: requested,
                        ..
                    }
                )
            )] if requested == &thread_id
        ));
    }

    #[gpui::test]
    fn viewport_effect_pumping_is_typed_and_rejects_stale_bottom_scroll(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let thread_id = ThreadId::parse("viewport-pump-thread").expect("thread");
        let host = cx.update(|_, app| {
            ConversationHost::mount(thread_id, ThemeMode::Dark, app).expect("host")
        });
        let generation = cx.update(|_, app| host.read(app).controller_view().viewport_generation);
        let stale_generation = ViewportGeneration::new(generation.value().saturating_add(1));
        cx.update(|_, app| {
            host.update(app, |host, _| {
                let _ = host.drain_effects();
            });
        });

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.state = NativeViewState::Ready;
                application.conversation_host = Some(host.clone());
                application.conversation_effects = vec![
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        ViewportEffect::ShowJumpToLatest,
                    )),
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        ViewportEffect::HideJumpToLatest,
                    )),
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        ViewportEffect::None,
                    )),
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        ViewportEffect::InvalidateRender,
                    )),
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        ViewportEffect::CompletionRejected {
                            generation,
                            reason: CompletionRejection::NoActiveScroll,
                        },
                    )),
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        ViewportEffect::RequestBottomScroll { generation },
                    )),
                    ConversationHostEffect::Controller(ConversationStateEffect::Viewport(
                        ViewportEffect::RequestBottomScroll {
                            generation: stale_generation,
                        },
                    )),
                ];

                application.pump_host_boundary(&host, application_cx);

                assert!(application.conversation_effects.is_empty());
                assert!(matches!(&application.state, NativeViewState::Ready));
            });
        });
    }

    #[gpui::test]
    fn scroll_intent_pumping_preserves_controller_view_without_completion(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let thread_id = ThreadId::parse("scroll-intent-thread").expect("thread");
        let host = cx.update(|_, app| {
            ConversationHost::mount(thread_id, ThemeMode::Dark, app).expect("host")
        });
        let surface = cx.update(|_, app| host.read(app).surface().clone());
        cx.update(|_, app| {
            host.update(app, |host, _| {
                let _ = host.drain_effects();
            });
        });
        let before = cx.update(|_, app| host.read(app).controller_view());
        let effect = ConversationHostEffect::ScrollIntent {
            target: ConversationSurfaceTarget::Scene(
                SceneId::parse("scroll-target").expect("scene id"),
            ),
        };

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.state = NativeViewState::Ready;
                application.conversation_host = Some(host.clone());
                application.conversation_effects = vec![effect.clone()];
                application.pump_host_boundary(&host, application_cx);

                assert!(application.conversation_effects.is_empty());
                assert!(matches!(&application.state, NativeViewState::Ready));
                assert_eq!(host.read(application_cx).controller_view(), before);
                assert!(host.read(application_cx).pending_effects().is_empty());
                assert!(surface.read(application_cx).pending_actions().is_empty());
            });
        });
    }

    #[gpui::test]
    fn scroll_intent_pumping_retains_fifo_head_when_surface_is_full(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let thread_id = ThreadId::parse("scroll-backpressure-thread").expect("thread");
        let host = cx.update(|_, app| {
            ConversationHost::mount(thread_id, ThemeMode::Dark, app).expect("host")
        });
        let surface = cx.update(|_, app| host.read(app).surface().clone());
        cx.update(|_, app| {
            host.update(app, |host, _| {
                let _ = host.drain_effects();
            });
            surface.update(app, |surface, surface_cx| {
                for index in 0..CONVERSATION_SURFACE_MAX_SCROLL_TARGETS {
                    assert!(surface.schedule_scroll_target(
                        ConversationSurfaceTarget::Scene(
                            SceneId::parse(format!("queued-{index}")).expect("scene id"),
                        ),
                        surface_cx,
                    ));
                }
            });
        });
        let effect = ConversationHostEffect::ScrollIntent {
            target: ConversationSurfaceTarget::Scene(
                SceneId::parse("backpressure-head").expect("scene id"),
            ),
        };

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.state = NativeViewState::Ready;
                application.conversation_host = Some(host.clone());
                application.conversation_effects = vec![effect.clone()];
                application.pump_host_boundary(&host, application_cx);

                assert_eq!(application.conversation_effects.as_slice(), &[effect]);
                assert!(matches!(&application.state, NativeViewState::Ready));
            });
        });
    }

    #[gpui::test]
    fn host_retirement_drops_pending_transient_scroll_target_with_surface(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let thread_id = ThreadId::parse("scroll-retirement-thread").expect("thread");
        let host = cx.update(|_, app| {
            ConversationHost::mount(thread_id, ThemeMode::Dark, app).expect("host")
        });
        let surface = cx.update(|_, app| host.read(app).surface().clone());
        let weak_surface = surface.downgrade();
        cx.update(|_, app| {
            host.update(app, |host, _| {
                let _ = host.drain_effects();
            });
            surface.update(app, |surface, surface_cx| {
                assert!(surface.schedule_scroll_target(
                    ConversationSurfaceTarget::Scene(
                        SceneId::parse("retiring-target").expect("scene id"),
                    ),
                    surface_cx,
                ));
            });
            view.update(app, |application, application_cx| {
                application.state = NativeViewState::Ready;
                application.conversation_host = Some(host.clone());
                application.retire_host(application_cx);
                assert!(application.conversation_host.is_none());
                assert!(application.conversation_effects.is_empty());
            });
        });
        drop(surface);
        drop(host);
        // Entity-data release runs at the end of the App update cycle:
        // dropping the last host handle queues host removal, and only the
        // cycle drops the host value that holds the final surface handle.
        // Queue cleanup alone cannot release real surface custody, so run an
        // update cycle before asserting it.
        cx.update(|_, _| {});
        assert!(weak_surface.upgrade().is_none());
    }

    #[gpui::test]
    fn ordinary_mount_boundary_retains_ready_host_without_replacement(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("forge-thread").expect("thread");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let snapshot = ConversationSnapshot::new(
            thread_id.clone(),
            ConversationCursor::new(0),
            Vec::new(),
            Vec::new(),
            UnixMillis::EPOCH,
        )
        .expect("empty snapshot");
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.pending_thread = Some(thread_id.clone());
                application.try_mount_pending_thread(application_cx);
                let host = application.conversation_host.clone().expect("mounted host");

                // The test has no service thread to accept the host's initial
                // request, so model that already-accepted command before
                // exercising the ordinary no-replacement boundary.
                application.conversation_effects.clear();
                application.dispatch_snapshot(&host, snapshot, application_cx);
                assert!(matches!(&application.state, NativeViewState::Ready));
                assert!(
                    host.read(application_cx)
                        .controller_view()
                        .delivery
                        .has_snapshot
                );
                assert!(application.conversation_host_subscription.is_some());

                application.try_mount_pending_thread(application_cx);

                assert_eq!(application.conversation_host.as_ref(), Some(&host));
                assert_eq!(application.selected_thread.as_ref(), Some(&thread_id));
                assert!(
                    host.read(application_cx)
                        .controller_view()
                        .delivery
                        .has_snapshot
                );
            });
        });
    }

    #[gpui::test]
    fn application_root_renders_without_a_service_thread(cx: &mut TestAppContext) {
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.run_until_parked();
        cx.update(|app| {
            assert!(view.read(app).service.is_none());
            assert!(matches!(&view.read(app).state, NativeViewState::Failure(_)));
        });
    }

    #[gpui::test]
    fn exact_snapshot_received_event_is_dispatched_to_the_real_host(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("forge-thread").expect("thread");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let snapshot = ConversationSnapshot::new(
            thread_id.clone(),
            ConversationCursor::new(0),
            Vec::new(),
            Vec::new(),
            UnixMillis::EPOCH,
        )
        .expect("empty snapshot");
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.pending_thread = Some(thread_id.clone());
                application.try_mount_pending_thread(application_cx);
                let host = application.conversation_host.clone().expect("real host");
                application.dispatch_snapshot(&host, snapshot, application_cx);
                assert!(
                    host.read(application_cx)
                        .controller_view()
                        .delivery
                        .has_snapshot
                );
            });
        });
    }

    fn first_receipt(
        request_id: &str,
        thread_id: &ThreadId,
        message_id: &str,
        disposition: ReceiptDisposition,
    ) -> QueueMessageReceipt {
        QueueMessageReceipt {
            request_id: artisan_domain::RequestId::parse(request_id).expect("request"),
            message_id: artisan_domain::MessageId::parse(message_id).expect("message"),
            thread_id: thread_id.clone(),
            disposition,
        }
    }

    #[test]
    fn one_domain_body_parse_admits_one_single_flight_and_retains_raw_text() {
        let mut composer = ComposerState::new();
        let raw = "  exact\n\t😀  ";
        composer.set_draft(raw);
        let (body, token) = composer.begin_submission().expect("valid body");
        assert_eq!(body.as_str(), raw);
        assert_eq!(
            composer.begin_submission(),
            Err(crate::composer::SubmissionBlocked::InFlight)
        );
        composer.finish_submission(token, DraftDisposition::Retained);
        assert_eq!(composer.draft(), raw);
        assert!(!composer.is_submitting());
    }

    #[test]
    fn each_new_message_submission_mints_a_fresh_request_id() {
        let first = create_message_request_id().expect("first request");
        let second = create_message_request_id().expect("second request");
        assert_ne!(first, second);
        assert!(first.as_str().starts_with("native-message-"));
        assert!(second.as_str().starts_with("native-message-"));
    }

    #[gpui::test]
    fn correlated_failure_retains_exact_retry_identity_and_body(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("retry-thread").expect("thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id.clone(),
                    "  exact retry body\n😀  ",
                    sink,
                );
                application.begin_message_submission(application_cx);
                let flight = application
                    .message_flight
                    .as_ref()
                    .expect("admitted flight");
                let request_id = flight.request_id.clone();
                let body = flight
                    .payload
                    .text()
                    .expect("text payload")
                    .as_str()
                    .to_owned();

                application.handle_service_event(
                    NativeTransportEvent::MessageFailed {
                        thread_id: thread_id.clone(),
                        request_id: request_id.clone(),
                        failure: message_failure(),
                    },
                    application_cx,
                );

                assert!(application.message_flight.is_none());
                let retry = application.message_retry.as_ref().expect("retry record");
                assert_eq!(retry.thread_id, thread_id);
                assert_eq!(retry.request_id, request_id);
                assert_eq!(retry.payload.text().expect("text payload").as_str(), body);
                assert_eq!(application.composer.read(application_cx).draft(), body);
            });
        });
        cx.run_until_parked();

        assert_eq!(commands.borrow().len(), 1);
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(
                application
                    .message_retry
                    .as_ref()
                    .is_some_and(|retry| retry.draft_matches)
            );
            assert!(!application.composer.read(app).is_submitting());
        });
    }

    #[gpui::test]
    fn retry_button_is_labeled_focused_and_has_deterministic_tab_stop(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("retry-button-thread").expect("thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id,
                    "retry button body",
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-button-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        assert_eq!(NATIVE_MESSAGE_RETRY_LABEL, "Retry send");
        // Mounting the pre-port message panel used to refresh the retry
        // focus handle through the builder; the panel is retired, so call
        // the builder directly — the same code the panel ran.
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                let _ = application.message_retry_button(application_cx);
            });
        });
        // The pre-port message panel is retired; the retry affordance
        // re-homes onto the legacy thread composer in a later packet. The
        // focus/tab-stop and ring-visibility contracts below remain.
        let ring_visible = cx.update(|window, app| {
            let application = view.read(app);
            assert_eq!(application.message_retry_focus_handle.tab_index, 2);
            assert!(application.message_retry_focus_handle.tab_stop);
            let focus = application.message_retry_focus_handle.clone();
            window.focus(&focus, app);
            Button::new(
                NATIVE_MESSAGE_RETRY_SELECTOR,
                focus,
                ArtisanTheme::for_mode(ThemeMode::Dark),
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::Small,
                ButtonContent::text(NATIVE_MESSAGE_RETRY_LABEL),
            )
            .expect("retry button configuration")
            .focus_visibility(FocusVisibility::Visible)
            .focus_ring_visible(window)
        });
        assert!(ring_visible, "retry action must expose visible focus");

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_draft("edited retry body");
                        composer_cx.notify();
                    });
            });
        });
        cx.run_until_parked();
        // The retired panel re-ran the builder on re-render, which is what
        // dropped the tab stop when the draft no longer matched; mirror it.
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                let _ = application.message_retry_button(application_cx);
            });
        });
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(application.message_retry.is_some());
            assert!(
                !application
                    .message_retry
                    .as_ref()
                    .is_some_and(|retry| retry.draft_matches)
            );
            assert!(!application.message_retry_focus_handle.tab_stop);
        });
    }

    #[gpui::test]
    fn pointer_enter_and_space_retry_activation_each_queue_once_with_stable_identity(
        cx: &mut TestAppContext,
    ) {
        let thread_id = ThreadId::parse("retry-activation-thread").expect("thread");
        let request_id = request("retry-stable-request");
        let body = "retry activation body";
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Ok(()), Ok(()), Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id.clone(),
                    body,
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-stable-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        // The painted retry control is retired; drive the same activation
        // handler its button invoked. Input-surface activation (Enter/Space
        // on the focused control) re-homes with the legacy message panel.
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_message_retry(application_cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(commands.borrow().len(), 1);

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_message_retry(application_cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(commands.borrow().len(), 2);

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_message_retry(application_cx);
            });
        });
        cx.run_until_parked();

        let commands = commands.borrow();
        assert_eq!(commands.len(), 3);
        for command in commands.iter() {
            let NativeTransportCommand::QueueMessage(command) = command else {
                panic!("retry activation must queue a first message")
            };
            assert_eq!(command.request_id, request_id);
            assert_eq!(command.thread_id, thread_id);
            assert_eq!(command.payload.text().expect("text payload").as_str(), body);
        }
    }

    #[gpui::test]
    fn retry_receipts_settle_only_matching_flights_and_stale_results_are_inert(
        cx: &mut TestAppContext,
    ) {
        let thread_id = ThreadId::parse("retry-receipt-thread").expect("thread");
        let stale_thread_id = ThreadId::parse("retry-stale-thread").expect("stale thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Ok(()), Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id.clone(),
                    "accepted retry body",
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-accepted-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_message_retry(application_cx);
                assert!(application.message_flight.is_some());
                application.handle_service_event(
                    NativeTransportEvent::MessageQueued(first_receipt(
                        "retry-stale-request",
                        &stale_thread_id,
                        "message-stale",
                        ReceiptDisposition::Accepted,
                    )),
                    application_cx,
                );
                application.handle_service_event(
                    NativeTransportEvent::MessageFailed {
                        thread_id: stale_thread_id,
                        request_id: request("retry-stale-request"),
                        failure: message_failure(),
                    },
                    application_cx,
                );
                assert!(application.message_flight.is_some());
                assert!(application.message_retry.is_none());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "accepted retry body"
                );
                application.handle_service_event(
                    NativeTransportEvent::MessageQueued(first_receipt(
                        "retry-accepted-request",
                        &thread_id,
                        "message-accepted",
                        ReceiptDisposition::Accepted,
                    )),
                    application_cx,
                );
                assert!(application.message_flight.is_none());
                assert_eq!(application.composer.read(application_cx).draft(), "");

                application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("duplicate retry body");
                        composer_cx.notify();
                    });
                application.message_receipt = None;
                application.message_failure = None;
                admit_message_flight(application, application_cx, "retry-duplicate-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_message_retry(application_cx);
                application.handle_service_event(
                    NativeTransportEvent::MessageQueued(first_receipt(
                        "retry-duplicate-request",
                        &thread_id,
                        "message-duplicate",
                        ReceiptDisposition::Duplicate,
                    )),
                    application_cx,
                );
                assert!(application.message_flight.is_none());
                assert_eq!(application.composer.read(application_cx).draft(), "");
            });
        });
        cx.run_until_parked();
        assert_eq!(commands.borrow().len(), 2);
    }

    #[gpui::test]
    fn edited_retry_is_suppressed_while_fresh_send_mints_a_new_request(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("retry-edited-thread").expect("thread");
        let original_request = request("retry-edited-request");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id.clone(),
                    "original retry body",
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-edited-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_draft("edited fresh body");
                        composer_cx.notify();
                    });
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_message_retry(application_cx);
                assert_eq!(commands.borrow().len(), 0);
                assert!(application.message_flight.is_none());
                assert!(application.message_retry.is_some());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "edited fresh body"
                );

                application.begin_message_submission(application_cx);
                assert!(application.message_retry.is_none());
                let flight = application.message_flight.as_ref().expect("fresh flight");
                assert_ne!(flight.request_id, original_request);
                assert_eq!(flight.thread_id, thread_id);
                assert_eq!(
                    flight.payload.text().expect("text payload").as_str(),
                    "edited fresh body"
                );
            });
        });

        let commands = commands.borrow();
        assert_eq!(commands.len(), 1);
        let NativeTransportCommand::QueueMessage(command) = &commands[0] else {
            panic!("fresh send must queue a first message")
        };
        assert_ne!(command.request_id, original_request);
        assert_eq!(command.thread_id, thread_id);
        assert_eq!(
            command.payload.text().expect("text payload").as_str(),
            "edited fresh body"
        );
    }

    #[gpui::test]
    fn busy_retry_admission_retains_identity_and_draft_without_a_flight(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("retry-busy-thread").expect("thread");
        let request_id = request("retry-busy-request");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Err(super::CommandSendError::Busy)]);
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id.clone(),
                    "busy retry body",
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-busy-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_message_retry(application_cx);
                let retry = application.message_retry.as_ref().expect("retained retry");
                assert_eq!(retry.thread_id, thread_id);
                assert_eq!(retry.request_id, request_id);
                assert_eq!(
                    retry.payload.text().expect("text payload").as_str(),
                    "busy retry body"
                );
                assert!(application.message_flight.is_none());
                assert!(!application.composer.read(application_cx).is_submitting());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "busy retry body"
                );
                assert!(matches!(
                    application.message_failure,
                    Some(NativeMessageFailure { failure, .. })
                        if failure.stage == super::ServiceFailureStage::EventBridge
                            && failure.category == super::ServiceFailureCategory::Backpressure
                ));
            });
        });
        assert_eq!(commands.borrow().len(), 1);
    }

    #[gpui::test]
    fn stopped_retry_admission_fails_closed_and_removes_the_affordance(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("retry-stopped-thread").expect("thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Err(super::CommandSendError::Stopped)]);
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id,
                    "stopped retry body",
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-stopped-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.activate_message_retry(application_cx);
                assert!(application.message_retry.is_none());
                assert!(application.message_flight.is_none());
                assert!(application.service_stopped);
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "stopped retry body"
                );
                assert!(!application.message_retry_focus_handle.tab_stop);
            });
        });
        assert_eq!(commands.borrow().len(), 1);
    }

    #[gpui::test]
    fn service_stop_event_clears_retry_while_retaining_the_draft(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("retry-stop-event-thread").expect("thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id,
                    "stop event draft",
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-stop-event-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                assert!(application.message_retry.is_some());
                application.handle_service_event(
                    NativeTransportEvent::Stopped(ServiceStopStatus::Clean),
                    application_cx,
                );
                assert!(application.message_retry.is_none());
                assert!(application.service_stopped);
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "stop event draft"
                );
            });
        });
    }

    #[gpui::test]
    fn thread_transition_clears_retry_while_retaining_the_draft(cx: &mut TestAppContext) {
        let project_id = ProjectId::parse("retry-transition-project").expect("project");
        let source = ThreadId::parse("retry-transition-source").expect("source");
        let target = ThreadId::parse("retry-transition-target").expect("target");
        let listing = ThreadListing::new(vec![
            thread(source.as_str(), "retry-transition-project", "Source"),
            thread(target.as_str(), "retry-transition-project", "Target"),
        ])
        .expect("listing");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, _) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    source.clone(),
                    "thread transition draft",
                    sink,
                );
                application.selected_project = Some(project_id.clone());
                application.thread_listing = Some(listing.clone());
                admit_message_flight(application, application_cx, "retry-transition-request");
                fail_active_message(application, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.begin_thread_switch(target, application_cx);
                assert!(application.message_retry.is_none());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "thread transition draft"
                );
                assert!(application.thread_switch_flight.is_some());
            });
        });
    }

    #[gpui::test]
    fn project_transition_clears_retry_without_losing_draft(cx: &mut TestAppContext) {
        let old_project = ProjectId::parse("retry-old-project").expect("old project");
        let new_project = ProjectId::parse("retry-new-project").expect("new project");
        let thread_id = ThreadId::parse("retry-project-thread").expect("thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id.clone(),
                    "project transition draft",
                    sink,
                );
                application.selected_project = Some(old_project.clone());
                admit_message_flight(application, application_cx, "retry-project-request");
                fail_active_message(application, application_cx);
                application.conversation_host = None;
                let projects =
                    ProjectListing::new(vec![project("retry-new-project", "New project")])
                        .expect("project listing");
                application.handle_projects(&projects, application_cx);
                assert!(application.message_retry.is_none());
                assert_eq!(application.selected_project, Some(new_project));
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "project transition draft"
                );
            });
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn host_retirement_clears_retry_without_losing_draft(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("retry-host-thread").expect("thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id,
                    "host retirement draft",
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-host-request");
                fail_active_message(application, application_cx);
                application.retire_host(application_cx);
                assert!(application.message_retry.is_none());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "host retirement draft"
                );
            });
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    fn shutdown_clears_retry_while_preserving_the_draft(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("retry-shutdown-thread").expect("thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, _) = command_sink(Vec::<Result<(), super::CommandSendError>>::new());
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                install_ready_message_surface(
                    application,
                    application_cx,
                    thread_id,
                    "shutdown retry draft",
                    sink,
                );
                admit_message_flight(application, application_cx, "retry-shutdown-request");
                fail_active_message(application, application_cx);
                application.prepare_shutdown(application_cx);
                assert!(application.message_retry.is_none());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "shutdown retry draft"
                );
                assert!(!application.message_retry_focus_handle.tab_stop);
            });
        });
    }

    #[test]
    fn message_failure_presentation_contains_only_redacted_stage_and_category() {
        let detail =
            message_status_detail(None, Some(NativeMessageFailure::new(message_failure())))
                .expect("failure detail");
        assert_eq!(detail, "Send failed: request (peer).");
        for secret in [
            "body text",
            "retry-request-id",
            "https://forge.invalid",
            "credential-value",
            "peer detail",
        ] {
            assert!(!detail.contains(secret), "failure detail leaked {secret}");
        }
    }

    #[gpui::test]
    fn busy_and_stopped_admission_retains_the_draft_without_an_application_flight(
        cx: &mut TestAppContext,
    ) {
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                for (error, draft) in [
                    (
                        super::CommandSendError::Busy,
                        "busy admission body".to_owned(),
                    ),
                    (
                        super::CommandSendError::Stopped,
                        "stopped admission body".to_owned(),
                    ),
                ] {
                    let (_, token) = application
                        .composer
                        .update(application_cx, |composer, composer_cx| {
                            composer.set_disabled(false, composer_cx);
                            composer.set_draft(draft.clone());
                            composer.begin_payload_submission()
                        })
                        .expect("begin");
                    application.reject_message_submission(
                        token,
                        super::command_failure(error),
                        application_cx,
                    );
                    assert!(application.message_flight.is_none());
                    assert_eq!(application.composer.read(application_cx).draft(), draft);
                    assert!(!application.composer.read(application_cx).is_submitting());
                }
            });
        });
    }

    #[gpui::test]
    fn accepted_and_duplicate_receipts_clear_only_the_matching_flight(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("forge-thread").expect("thread");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.selected_thread = Some(thread_id.clone());
                application.state = NativeViewState::Ready;
                let (body, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("first exact body");
                        composer.begin_payload_submission()
                    })
                    .expect("first begin");
                application.message_flight = Some(NativeMessageFlight {
                    thread_id: thread_id.clone(),
                    request_id: artisan_domain::RequestId::parse("request-first").expect("request"),
                    payload: body,
                    token,
                });
                application.handle_service_event(
                    NativeTransportEvent::MessageQueued(first_receipt(
                        "request-first",
                        &thread_id,
                        "message-first",
                        ReceiptDisposition::Accepted,
                    )),
                    application_cx,
                );
                assert!(application.message_flight.is_none());
                assert_eq!(application.composer.read(application_cx).draft(), "");
                assert_eq!(
                    application
                        .message_receipt
                        .as_ref()
                        .map(|receipt| receipt.disposition),
                    Some(ReceiptDisposition::Accepted)
                );

                let (body, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("second exact body");
                        composer.begin_payload_submission()
                    })
                    .expect("second begin");
                application.message_flight = Some(NativeMessageFlight {
                    thread_id: thread_id.clone(),
                    request_id: artisan_domain::RequestId::parse("request-second")
                        .expect("request"),
                    payload: body,
                    token,
                });
                application.composer.update(application_cx, |composer, _| {
                    composer.set_draft("newer draft while duplicate is pending");
                });
                application.handle_service_event(
                    NativeTransportEvent::MessageQueued(first_receipt(
                        "request-second",
                        &thread_id,
                        "message-second",
                        ReceiptDisposition::Duplicate,
                    )),
                    application_cx,
                );
                assert!(application.message_flight.is_none());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "newer draft while duplicate is pending"
                );
                assert_eq!(
                    application
                        .message_receipt
                        .as_ref()
                        .map(|receipt| receipt.disposition),
                    Some(ReceiptDisposition::Duplicate)
                );
            });
        });
    }

    #[gpui::test]
    fn stale_queue_results_do_not_clear_a_newer_draft(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("forge-thread").expect("thread");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.selected_thread = Some(thread_id.clone());
                application.state = NativeViewState::Ready;
                let (body, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("newer draft");
                        composer.begin_payload_submission()
                    })
                    .expect("begin");
                application.message_flight = Some(NativeMessageFlight {
                    thread_id: thread_id.clone(),
                    request_id: artisan_domain::RequestId::parse("request-newer").expect("request"),
                    payload: body,
                    token,
                });
                application.handle_service_event(
                    NativeTransportEvent::MessageQueued(first_receipt(
                        "request-stale",
                        &thread_id,
                        "message-stale",
                        ReceiptDisposition::Accepted,
                    )),
                    application_cx,
                );
                assert!(application.message_flight.is_some());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "newer draft"
                );
            });
        });
    }

    #[gpui::test]
    fn queue_failure_and_service_stop_retain_the_draft(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("forge-thread").expect("thread");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.selected_thread = Some(thread_id.clone());
                application.state = NativeViewState::Ready;
                let (body, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("retained queue body");
                        composer.begin_payload_submission()
                    })
                    .expect("begin");
                application.message_flight = Some(NativeMessageFlight {
                    thread_id: thread_id.clone(),
                    request_id: artisan_domain::RequestId::parse("request-failure")
                        .expect("request"),
                    payload: body,
                    token,
                });
                application.handle_service_event(
                    NativeTransportEvent::MessageFailed {
                        thread_id: thread_id.clone(),
                        request_id: artisan_domain::RequestId::parse("request-failure")
                            .expect("request"),
                        failure: ServiceFailure {
                            stage: super::ServiceFailureStage::Request,
                            category: super::ServiceFailureCategory::Peer,
                        },
                    },
                    application_cx,
                );
                assert!(application.message_flight.is_none());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "retained queue body"
                );
                assert!(application.message_failure.is_some());

                let (body, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("retained on stop");
                        composer.begin_payload_submission()
                    })
                    .expect("second begin");
                application.message_flight = Some(NativeMessageFlight {
                    thread_id: thread_id.clone(),
                    request_id: artisan_domain::RequestId::parse("request-stop").expect("request"),
                    payload: body,
                    token,
                });
                application.handle_service_event(
                    NativeTransportEvent::Stopped(ServiceStopStatus::Clean),
                    application_cx,
                );
                assert!(application.message_flight.is_none());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "retained on stop"
                );
            });
        });
    }

    #[gpui::test]
    fn real_thread_transition_and_shutdown_retain_and_clear_old_presentation(
        cx: &mut TestAppContext,
    ) {
        let old_thread = ThreadId::parse("old-thread").expect("thread");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                application.selected_thread = Some(old_thread.clone());
                application.state = NativeViewState::Ready;
                let (body, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("transition body");
                        composer.begin_payload_submission()
                    })
                    .expect("begin");
                application.message_flight = Some(NativeMessageFlight {
                    thread_id: old_thread.clone(),
                    request_id: artisan_domain::RequestId::parse("request-transition")
                        .expect("request"),
                    payload: body,
                    token,
                });
                application.message_receipt = Some(first_receipt(
                    "request-old",
                    &old_thread,
                    "message-old",
                    ReceiptDisposition::Accepted,
                ));
                application.message_failure = Some(NativeMessageFailure::new(ServiceFailure {
                    stage: super::ServiceFailureStage::Request,
                    category: super::ServiceFailureCategory::Peer,
                }));
                application.retire_host(application_cx);
                assert!(application.message_flight.is_none());
                assert_eq!(application.selected_thread, None);
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "transition body"
                );
                assert!(application.message_receipt.is_none());
                assert!(application.message_failure.is_none());

                application.selected_thread = Some(old_thread.clone());
                application.state = NativeViewState::Ready;
                let (body, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("shutdown body");
                        composer.begin_payload_submission()
                    })
                    .expect("shutdown begin");
                application.message_flight = Some(NativeMessageFlight {
                    thread_id: old_thread,
                    request_id: artisan_domain::RequestId::parse("request-shutdown")
                        .expect("request"),
                    payload: body,
                    token,
                });
                application.prepare_shutdown(application_cx);
                assert!(application.message_flight.is_none());
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "shutdown body"
                );
                assert!(application.message_receipt.is_none());
                assert!(application.message_failure.is_none());
            });
        });
    }

    #[gpui::test]
    fn thread_switch_is_serial_and_rejects_old_generation_delivery(cx: &mut TestAppContext) {
        let project_id = ProjectId::parse("switch-project").expect("project");
        let source = ThreadId::parse("switch-thread-a").expect("source thread");
        let target = ThreadId::parse("switch-thread-b").expect("target thread");
        let listing = ThreadListing::new(vec![
            thread("switch-thread-a", "switch-project", "A"),
            thread("switch-thread-b", "switch-project", "B"),
        ])
        .expect("listing");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));

        cx.update(|app| {
            view.update(app, |application, application_cx| {
                let commands = prepare_thread_switch_fixture(
                    application,
                    application_cx,
                    &project_id,
                    &source,
                    &target,
                    &listing,
                );
                let stop_request = complete_first_thread_switch(
                    application,
                    application_cx,
                    &source,
                    &target,
                    &commands,
                );
                refresh_listing_and_reject_stale_snapshot(
                    application,
                    application_cx,
                    &project_id,
                    &source,
                    &listing,
                );
                return_to_source_and_reject_old_generation(
                    application,
                    application_cx,
                    &source,
                    &target,
                    &commands,
                    stop_request,
                );
            });
        });
    }

    fn prepare_thread_switch_fixture(
        application: &mut NativeApplication,
        application_cx: &mut Context<NativeApplication>,
        project_id: &ProjectId,
        source: &ThreadId,
        target: &ThreadId,
        listing: &ThreadListing,
    ) -> Rc<RefCell<Vec<NativeTransportCommand>>> {
        let source_host =
            ConversationHost::mount(source.clone(), ThemeMode::Dark, &mut *application_cx)
                .expect("source host");
        application.project_options = vec![ProjectOption {
            id: project_id.clone(),
            name: "Switch project".into(),
        }];
        application.selected_project = Some(project_id.clone());
        application.thread_listing = Some(listing.clone());
        application.selected_thread = Some(source.clone());
        application.conversation_host = Some(source_host.clone());
        application.active_subscription_request_id = Some(request("switch-start-a-1"));
        application.state = NativeViewState::Ready;

        let (body, token) = application
            .composer
            .update(application_cx, |composer, composer_cx| {
                composer.set_disabled(false, composer_cx);
                composer.switch_thread(source.as_str().to_owned(), false, composer_cx);
                composer.set_draft("retained switch draft");
                composer.begin_payload_submission()
            })
            .expect("message flight");
        application.message_flight = Some(NativeMessageFlight {
            thread_id: source.clone(),
            request_id: request("message-switch"),
            payload: body,
            token,
        });

        let (sink, commands) = command_sink([Ok(()), Ok(()), Ok(()), Ok(())]);
        application.test_command_sink = Some(sink);
        application.install_thread_picker(listing.clone(), Some(source.clone()), application_cx);

        application.begin_thread_switch(target.clone(), application_cx);
        assert!(matches!(
            application
                .thread_switch_flight
                .as_ref()
                .map(|flight| &flight.phase),
            Some(ThreadSwitchPhase::AwaitingUnsubscribeStop { request_id: None })
        ));
        assert_eq!(commands.borrow().len(), 1);
        assert!(matches!(
            &commands.borrow()[0],
            NativeTransportCommand::Unsubscribe { thread_id } if thread_id == source
        ));
        assert_eq!(application.conversation_host.as_ref(), Some(&source_host));
        assert_eq!(application.selected_thread.as_ref(), Some(source));
        assert!(
            application
                .thread_picker
                .as_ref()
                .expect("thread picker")
                .read(application_cx)
                .state()
                .is_disabled()
        );
        assert_eq!(
            application.composer.read(application_cx).draft(),
            "retained switch draft"
        );
        commands
    }

    fn complete_first_thread_switch(
        application: &mut NativeApplication,
        application_cx: &mut Context<NativeApplication>,
        source: &ThreadId,
        target: &ThreadId,
        commands: &Rc<RefCell<Vec<NativeTransportCommand>>>,
    ) -> RequestId {
        let stop_request = request("switch-stop-a-1");
        application.handle_service_event(
            NativeTransportEvent::ConversationSubscriptionStopped {
                thread_id: source.clone(),
                request_id: stop_request.clone(),
                stopped: ConversationSubscriptionStopped {
                    thread_id: source.clone(),
                },
            },
            application_cx,
        );
        assert_eq!(commands.borrow().len(), 2);
        assert!(matches!(
            &commands.borrow()[1],
            NativeTransportCommand::Subscribe {
                thread_id,
                after: None,
            } if thread_id == target
        ));
        assert_eq!(application.selected_thread.as_ref(), Some(target));
        assert_eq!(
            application
                .conversation_host
                .as_ref()
                .expect("target host")
                .read(application_cx)
                .controller_view()
                .delivery
                .thread_id,
            *target
        );
        assert!(matches!(
            application
                .thread_switch_flight
                .as_ref()
                .map(|flight| &flight.phase),
            Some(ThreadSwitchPhase::AwaitingSubscriptionStart { request_id: None })
        ));
        assert!(application.conversation_effects.is_empty());

        application.handle_service_event(
            fresh_start_event(target, "switch-start-b-1", 1),
            application_cx,
        );
        assert!(application.thread_switch_flight.is_none());
        assert_eq!(application.selected_thread.as_ref(), Some(target));
        assert!(matches!(&application.state, NativeViewState::Ready));
        assert!(
            application
                .conversation_host
                .as_ref()
                .expect("started target host")
                .read(application_cx)
                .controller_view()
                .delivery
                .has_snapshot
        );
        assert_eq!(commands.borrow().len(), 2);
        // Each thread owns its draft; A's pending text must not leak into B.
        assert_eq!(application.composer.read(application_cx).draft(), "");
        stop_request
    }

    fn refresh_listing_and_reject_stale_snapshot(
        application: &mut NativeApplication,
        application_cx: &mut Context<NativeApplication>,
        project_id: &ProjectId,
        source: &ThreadId,
        listing: &ThreadListing,
    ) {
        let refreshed_listing = ThreadListing::new(vec![
            thread("switch-thread-a", "switch-project", "A refreshed"),
            thread("switch-thread-b", "switch-project", "B refreshed"),
        ])
        .expect("refreshed listing");
        application.handle_service_event(
            NativeTransportEvent::Threads {
                project_id: project_id.clone(),
                listing: refreshed_listing.clone(),
            },
            application_cx,
        );
        assert_eq!(
            application.thread_listing.as_ref(),
            Some(&refreshed_listing)
        );
        application.handle_service_event(
            NativeTransportEvent::Threads {
                project_id: project_id.clone(),
                listing: listing.clone(),
            },
            application_cx,
        );
        assert_eq!(
            application.thread_listing.as_ref(),
            Some(&refreshed_listing)
        );

        let target_host = application.conversation_host.clone().expect("target host");
        application.handle_service_event(
            NativeTransportEvent::Snapshot(snapshot_for(source, 99)),
            application_cx,
        );
        assert_eq!(application.conversation_host.as_ref(), Some(&target_host));
        assert_eq!(application.pending_snapshot, None);
        assert!(matches!(&application.state, NativeViewState::Ready));
    }

    fn return_to_source_and_reject_old_generation(
        application: &mut NativeApplication,
        application_cx: &mut Context<NativeApplication>,
        source: &ThreadId,
        target: &ThreadId,
        commands: &Rc<RefCell<Vec<NativeTransportCommand>>>,
        stop_request: RequestId,
    ) {
        application.begin_thread_switch(source.clone(), application_cx);
        assert_eq!(commands.borrow().len(), 3);
        application.handle_service_event(
            NativeTransportEvent::ConversationSubscriptionStopped {
                thread_id: target.clone(),
                request_id: request("switch-stop-b-1"),
                stopped: ConversationSubscriptionStopped {
                    thread_id: target.clone(),
                },
            },
            application_cx,
        );
        assert_eq!(commands.borrow().len(), 4);
        assert!(matches!(
            &commands.borrow()[3],
            NativeTransportCommand::Subscribe {
                thread_id,
                after: None,
            } if thread_id == source
        ));
        application.handle_service_event(
            fresh_start_event(source, "switch-start-a-2", 2),
            application_cx,
        );
        let returned_host = application
            .conversation_host
            .clone()
            .expect("returned host");
        assert!(application.thread_switch_flight.is_none());
        assert_eq!(application.selected_thread.as_ref(), Some(source));
        assert_eq!(
            returned_host
                .read(application_cx)
                .controller_view()
                .delivery
                .cursor,
            Some(ConversationCursor::new(2))
        );

        // The first A start and stop receipts belong to the older
        // generation. Neither may displace the current A host.
        application.handle_service_event(
            fresh_start_event(source, "switch-start-a-1", 99),
            application_cx,
        );
        application.handle_service_event(
            NativeTransportEvent::ConversationSubscriptionStopped {
                thread_id: source.clone(),
                request_id: stop_request,
                stopped: ConversationSubscriptionStopped {
                    thread_id: source.clone(),
                },
            },
            application_cx,
        );
        assert_eq!(application.conversation_host.as_ref(), Some(&returned_host));
        assert_eq!(application.selected_thread.as_ref(), Some(source));
        assert_eq!(commands.borrow().len(), 4);
        assert_eq!(
            application.composer.read(application_cx).draft(),
            "retained switch draft"
        );
    }

    #[gpui::test]
    fn thread_switch_busy_is_retried_once_without_duplicate_admission(cx: &mut TestAppContext) {
        let project_id = ProjectId::parse("busy-project").expect("project");
        let source = ThreadId::parse("busy-thread-a").expect("source thread");
        let target = ThreadId::parse("busy-thread-b").expect("target thread");
        let listing = ThreadListing::new(vec![
            thread("busy-thread-a", "busy-project", "A"),
            thread("busy-thread-b", "busy-project", "B"),
        ])
        .expect("listing");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                let source_host =
                    ConversationHost::mount(source.clone(), ThemeMode::Dark, &mut *application_cx)
                        .expect("source host");
                application.project_options = vec![ProjectOption {
                    id: project_id.clone(),
                    name: "Busy project".into(),
                }];
                application.selected_project = Some(project_id);
                application.thread_listing = Some(listing);
                application.selected_thread = Some(source);
                application.conversation_host = Some(source_host);
                application.state = NativeViewState::Ready;
                let (sink, commands) = command_sink([Err(super::CommandSendError::Busy), Ok(())]);
                application.test_command_sink = Some(sink);
                application.begin_thread_switch(target, application_cx);
                assert_eq!(commands.borrow().len(), 1);
                assert!(matches!(
                    application
                        .thread_switch_flight
                        .as_ref()
                        .map(|flight| &flight.phase),
                    Some(ThreadSwitchPhase::UnsubscribeAdmission {
                        retry_pending: true,
                        retry_used: true,
                    })
                ));
                application.retry_thread_switch_if_admitted(application_cx);
                assert_eq!(commands.borrow().len(), 2);
                assert!(matches!(
                    application
                        .thread_switch_flight
                        .as_ref()
                        .map(|flight| &flight.phase),
                    Some(ThreadSwitchPhase::AwaitingUnsubscribeStop { request_id: None })
                ));
                application.retry_thread_switch_if_admitted(application_cx);
                assert_eq!(commands.borrow().len(), 2);
            });
        });
    }

    #[gpui::test]
    fn terminal_switch_refusal_preserves_old_host_and_disables_picker(cx: &mut TestAppContext) {
        let project_id = ProjectId::parse("stopped-project").expect("project");
        let source = ThreadId::parse("stopped-thread-a").expect("source thread");
        let target = ThreadId::parse("stopped-thread-b").expect("target thread");
        let listing = ThreadListing::new(vec![
            thread("stopped-thread-a", "stopped-project", "A"),
            thread("stopped-thread-b", "stopped-project", "B"),
        ])
        .expect("listing");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                let source_host =
                    ConversationHost::mount(source.clone(), ThemeMode::Dark, &mut *application_cx)
                        .expect("source host");
                application.project_options = vec![ProjectOption {
                    id: project_id.clone(),
                    name: "Stopped project".into(),
                }];
                application.selected_project = Some(project_id);
                application.thread_listing = Some(listing.clone());
                application.selected_thread = Some(source.clone());
                application.conversation_host = Some(source_host.clone());
                application.state = NativeViewState::Ready;
                let (body, token) = application
                    .composer
                    .update(application_cx, |composer, composer_cx| {
                        composer.set_disabled(false, composer_cx);
                        composer.set_draft("refused switch draft");
                        composer.begin_payload_submission()
                    })
                    .expect("message flight");
                application.message_flight = Some(NativeMessageFlight {
                    thread_id: source.clone(),
                    request_id: request("message-stopped"),
                    payload: body,
                    token,
                });
                let (sink, commands) = command_sink([Err(super::CommandSendError::Stopped)]);
                application.test_command_sink = Some(sink);
                application.install_thread_picker(listing, Some(source.clone()), application_cx);
                application.begin_thread_switch(target, application_cx);
                assert_eq!(commands.borrow().len(), 1);
                assert!(application.thread_switch_flight.is_none());
                assert!(application.service_stopped);
                assert_eq!(application.conversation_host.as_ref(), Some(&source_host));
                assert_eq!(application.selected_thread.as_ref(), Some(&source));
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "refused switch draft"
                );
                assert!(matches!(&application.state, NativeViewState::Failure(_)));
                assert!(
                    application
                        .thread_picker
                        .as_ref()
                        .expect("thread picker")
                        .read(application_cx)
                        .state()
                        .is_disabled()
                );
            });
        });
    }

    #[gpui::test]
    fn removed_switch_target_retires_without_subscribing_it(cx: &mut TestAppContext) {
        let project_id = ProjectId::parse("removed-project").expect("project");
        let source = ThreadId::parse("removed-thread-a").expect("source thread");
        let target = ThreadId::parse("removed-thread-b").expect("target thread");
        let listing = ThreadListing::new(vec![
            thread("removed-thread-a", "removed-project", "A"),
            thread("removed-thread-b", "removed-project", "B"),
        ])
        .expect("listing");
        let remaining =
            ThreadListing::new(vec![thread("removed-thread-a", "removed-project", "A")])
                .expect("remaining listing");
        let (view, _) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|app| {
            view.update(app, |application, application_cx| {
                let source_host =
                    ConversationHost::mount(source.clone(), ThemeMode::Dark, &mut *application_cx)
                        .expect("source host");
                application.project_options = vec![ProjectOption {
                    id: project_id.clone(),
                    name: "Removed project".into(),
                }];
                application.selected_project = Some(project_id.clone());
                application.thread_listing = Some(listing.clone());
                application.selected_thread = Some(source.clone());
                application.conversation_host = Some(source_host);
                application.state = NativeViewState::Ready;
                let (sink, commands) = command_sink([Ok(())]);
                application.test_command_sink = Some(sink);
                application.begin_thread_switch(target, application_cx);
                application.handle_service_event(
                    NativeTransportEvent::Threads {
                        project_id,
                        listing: remaining,
                    },
                    application_cx,
                );
                assert!(matches!(
                    application
                        .thread_switch_flight
                        .as_ref()
                        .map(|flight| &flight.target_thread),
                    Some(None)
                ));
                application.handle_service_event(
                    NativeTransportEvent::ConversationSubscriptionStopped {
                        thread_id: source.clone(),
                        request_id: request("removed-stop-a"),
                        stopped: ConversationSubscriptionStopped { thread_id: source },
                    },
                    application_cx,
                );
                assert_eq!(commands.borrow().len(), 1);
                assert!(application.thread_switch_flight.is_none());
                assert!(application.conversation_host.is_none());
                assert!(application.selected_thread.is_none());
                assert!(matches!(&application.state, NativeViewState::EmptyThreads));
            });
        });
    }

    #[test]
    fn production_title_is_the_native_title() {
        assert_eq!(WINDOW_TITLE, "Artisan Editor");
        assert!(!WINDOW_TITLE.contains("phase"));
    }
}

#[path = "native_composer_run_controls.rs"]
mod composer_run_controls;

#[path = "native_composer_models.rs"]
mod native_composer_models;

#[path = "native_composer_queue_application.rs"]
mod composer_queue_application;
