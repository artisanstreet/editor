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
    collections::HashMap,
    process::ExitCode,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use artisan_assets::AssetId;
use artisan_domain::{
    CatalogRevision, ConversationItem, ConversationSnapshot, EngineProfileId, ModelFavoriteId,
    PatchBatch, ProjectId, ProjectListing, RequestId, SetModelFavorite, ThreadId, ThreadListing,
};
use artisan_protocol::{ConversationSubscriptionStarted, QueueMessageReceipt, ServerEvent};
use artisan_ui::asset_seam::asset_glyph;
#[cfg(test)]
use artisan_ui::button::{
    AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility,
};
use artisan_ui::dropdown_menu::{DropdownMenuEntry, DropdownMenuItem, DropdownMenuState};
use artisan_ui::fade_arc::FadeArc;
use artisan_ui::icon::{IconSize, IconStyle, IconTint, icon};
#[cfg(test)]
use artisan_ui::motion::MotionPolicy;
use artisan_ui::motion::{MotionCurve, MotionDuration};
use artisan_ui::theme::{
    ArtisanTheme, DesktopTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode,
};
use gpui::Focusable as _;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, ClipboardItem, Context, Div, Entity,
    FocusHandle, FontWeight, HighlightStyle, KeyBinding, Render, ScrollHandle, ScrollWheelEvent,
    SharedString, Stateful, StatefulInteractiveElement, StyledText, Subscription, Task,
    TitlebarOptions, Window, WindowBounds, WindowOptions, actions, canvas, div,
    prelude::{InteractiveElement as _, IntoElement, ParentElement as _, Styled as _},
    px, size,
};

use crate::composer::{DraftDisposition, SubmissionToken};
use crate::conversation_scene::TurnEngineLabel;
use crate::desktop_shell::{
    DESKTOP_COMPOSER_SELECTOR, DESKTOP_HOME_SELECTOR, DesktopShellStyle, desktop_nav_glyph,
    desktop_shell,
};
use crate::editor_route_screen::{EditorScreen, EditorScreenIdentity, EditorSurfaceState};
use crate::home_project_picker::{
    HOME_CHOOSE_PROJECT_LABEL, HOME_EMBLEM_SIZE_PX, HOME_HEADLINE_TEXT_PX, HomeProjectPickerView,
};
use crate::native_command_menu::{
    CommandMenuAction, CommandMenuEntry, CommandMenuGroup, NativeCommandMenu,
};
use crate::native_composer::{NativeComposer, NativeComposerEvent};
use crate::native_composer_controls::{
    NativeComposerControls, NativeComposerControlsEvent, NativeComposerControlsSnapshot,
};
use crate::native_composer_material::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use crate::native_message_images::{NativeMessageImages, NativeMessageImagesEvent};
use crate::native_model_catalog::NativeModelCatalog;
use crate::native_model_selector::{
    HoverRect, NativeModelSelector, NativeModelSelectorStatus, PICKER_MENU_MOTION_DURATION_MS,
    PickerMenuMotion, PickerMenuPhase, PickerScrollState, SlidingHoverState, animate_picker_menu,
    engine_accent, engine_asset, render_picker_hover_pill,
};
use crate::native_profile_usage::{
    NativeProfileUsageState, NativeUsageEntry, NativeUsageWindow, ProfileUsageGeneration,
    account_usage_response_current, checked_label, engine_readiness, engine_readiness_reason,
    engine_refresh_failure, group_usage_windows, plan_profile_usage_loads,
    profile_usage_display_name, reset_duration, tip_run_up_from, usage_remaining_percent,
};
use crate::native_route::{NativeRoute, RouteHistory, SettingsRoute};
use crate::native_settings::{
    SettingsEngineCatalogState, SettingsEngineModel, SettingsEngineNavEntry,
    SettingsEngineRegistryState, SettingsEngineSnapshot, SettingsScreen, SettingsScreenEvent,
};
use crate::native_transport::{
    CatalogLoadGeneration, CatalogScopeError, NativeCatalogController, NativeCatalogPhase,
    NativeCatalogScope,
};
use crate::native_transport_service::{
    CommandSendError, EventReceiveError, HoldKind, NativeProjectIntakeOperation,
    NativeProjectIntakeStage, NativeTransportCommand, NativeTransportEvent, NativeTransportService,
    ServiceFailure, ServiceFailureCategory, ServiceFailureStage, ServiceStopStatus,
    SettingsLoadGeneration,
};
use crate::onboarding_harness_presentation::{
    HarnessCatalog, HarnessSetupAction, HarnessSetupState,
};
use crate::onboarding_screen::{OnboardingHarnessEntry, OnboardingScreen};
use crate::repository_mark::repository_mark_for;
use crate::thread_environment_presentation::{HostIdentitySnapshot, ThreadEnvironmentInput};
use crate::thread_screen::{ThreadScreen, ThreadScreenGate, shell_black};
use crate::titlebar_header_presentation::{
    TITLEBAR_HEADER_THREAD_SEPARATOR, TitlebarHeaderInput, TitlebarHeaderSegment,
    TitlebarRepository, present_titlebar_header,
};
use crate::usage_meter::usage_segment_fraction;
use crate::workspace_tab_state::EditorViewState;
use crate::{
    conversation_delivery_machine::{ConversationDeliveryEffect, ConversationDeliveryEvent},
    conversation_host::{CONVERSATION_HOST_MAX_EFFECTS, ConversationHost, ConversationHostEffect},
    conversation_state_machine::{ConversationStateEffect, ConversationStateEvent},
    conversation_view_machine::ViewportState,
    engine_observation_state::{ApplyOutcome, EngineObservationState},
    engine_settings::{
        EngineSettingsController, EngineSettingsFailureOperation, RegistryView,
        manual_configuration_template,
    },
    native_thread_picker::{NativeThreadPicker, ThreadPickerAction},
    project_picker::{ProjectOption, ProjectPickerAction, ProjectPickerView},
};

/// The label of a new-thread route before its thread exists; native task
/// creation also writes it as the placeholder title the Forge later
/// resolves.
pub(crate) const UNNAMED_THREAD_TITLE: &str = "New thread";

// Phase-1 split submodules (see native_application/).

#[path = "native_application/selectors.rs"]
mod selectors;

#[path = "native_application/profile_motion.rs"]
mod profile_motion;

#[path = "native_application/state.rs"]
mod state;

#[path = "native_application/presentation.rs"]
mod presentation;

// Phase-2 split submodules (see native_application/).

mod host_switch;

mod composer_drafts;
#[path = "native_application/impl_lifecycle.rs"]
mod impl_lifecycle;
mod impl_machines;
mod machine_submenu;
mod workspace;
#[cfg(windows)]
mod wsl_project_picker;

#[path = "native_application/impl_profile_menu.rs"]
mod impl_profile_menu;

#[path = "native_application/impl_message_flight.rs"]
mod impl_message_flight;

#[path = "native_application/new_task_send.rs"]
mod new_task_send;

#[path = "native_application/impl_service_events.rs"]
mod impl_service_events;

#[path = "native_application/impl_answer_events.rs"]
mod impl_answer_events;

#[path = "native_application/impl_engine_settings.rs"]
mod impl_engine_settings;

// Phase-3 split submodules (see native_application/).

#[path = "native_application/impl_surfaces.rs"]
mod impl_surfaces;

#[path = "native_application/impl_pickers.rs"]
mod impl_pickers;

#[path = "native_application/impl_new_task.rs"]
mod impl_new_task;

#[path = "native_application/impl_sidebar_and_menus.rs"]
mod impl_sidebar_and_menus;

// Phase-4 split submodules (see native_application/).

#[path = "native_application/impl_rich_links.rs"]
mod impl_rich_links;

#[path = "native_application/impl_host_mounting.rs"]
mod impl_host_mounting;

#[path = "native_application/impl_catalog.rs"]
mod impl_catalog;

#[path = "native_application/impl_profile_usage.rs"]
mod impl_profile_usage;

#[path = "native_application/impl_engine_installs.rs"]
mod impl_engine_installs;

// Phase-5 split submodules (see native_application/).

#[path = "native_application/app_entry.rs"]
mod app_entry;
mod frame_capture;

#[path = "native_application/impl_route_surface.rs"]
mod impl_route_surface;

#[path = "native_application/impl_render.rs"]
mod impl_render;

#[path = "native_application/impl_failure.rs"]
mod impl_failure;

#[cfg(test)]
use app_entry::bind_native_actions;
pub use app_entry::run;

use selectors::{
    MAX_RETAINED_SWITCH_LISTINGS, MAX_RETAINED_SWITCH_PATCH_IDS, MAX_RETAINED_SWITCH_REQUEST_IDS,
    NATIVE_KEY_CONTEXT, NATIVE_ROOT_SELECTOR, POLL_INTERVAL, PROFILE_MENU_ANCHOR_GAP_PX,
    PROFILE_MENU_VIEWPORT_MARGIN_PX, PROFILE_SETTINGS_HOVER_ID, PROFILE_USAGE_HOVER_ID,
    SIDEBAR_MARKETPLACE_HOVER_ID, SIDEBAR_NEW_THREAD_HOVER_ID, SIDEBAR_PROFILE_HOVER_ID,
    SURFACE_HEIGHT, SURFACE_WIDTH, TITLEBAR_HEADER_SELECTOR, TITLEBAR_PROJECT_FOLDER_SELECTOR,
    TITLEBAR_REPOSITORY_LABEL_SELECTOR, TITLEBAR_REPOSITORY_MARK_SELECTOR,
    TITLEBAR_ROUTE_TITLE_SELECTOR, TITLEBAR_THREAD_SEPARATOR_SELECTOR,
};
#[cfg(test)]
use selectors::{
    NATIVE_RAIL_ADD_PROJECT_LABEL, NATIVE_RAIL_ADD_PROJECT_SELECTOR, NATIVE_STATUS_SELECTOR,
    WINDOW_TITLE,
};

use profile_motion::{
    PROFILE_MENU_FIXED_CHROME_PX, PROFILE_METER_TICK_PX, ProfileTipTween, RefreshSwap,
    RefreshSwapTarget, swap_offsets_for,
};

#[cfg(test)]
use state::NativeTestCommandSink;
use state::{
    NativeMessageFailure, NativeMessageFlight, NativeViewState, PickerRoute, ThreadSwitchFlight,
    ThreadSwitchPhase, command_failure, create_message_request_id, create_save_request_id,
    empty_thread_listing, intake_command, invalid_service_failure, mint_request_id, picker_route,
    project_options_from_listing, ready_membership_is_valid, submission_blocked_failure,
};

#[cfg(test)]
use presentation::message_status_detail;
use presentation::{
    capitalize_label, profile_usage_now_ms, repository_logo_asset, status_panel,
    titlebar_context_tone, titlebar_repository_for_project,
};

actions!(
    native_application,
    [
        Quit,
        NextTabStop,
        PreviousTabStop,
        OpenCommandMenu,
        OpenMachines,
        ToggleFrameCounter
    ]
);

/// Probe measuring one sidebar row against the shared sidebar hover
/// surface, so New thread, Marketplace, and the profile footer all drive
/// the same sliding pill from their actual bounds.
fn sidebar_hover_probe(
    hover: Rc<RefCell<SlidingHoverState>>,
    surface_bounds: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
    id: impl Into<String>,
) -> gpui::Canvas<()> {
    let measured_id = id.into();
    canvas(
        |_, _, _| {},
        move |bounds, (), window, cx| {
            let Some(surface) = *surface_bounds.borrow() else {
                return;
            };
            let rect = HoverRect {
                left: f32::from(bounds.left() - surface.left()),
                top: f32::from(bounds.top() - surface.top()),
                width: f32::from(bounds.size.width),
                height: f32::from(bounds.size.height),
            };
            if hover.borrow_mut().measure(&measured_id, rect) {
                window.defer(cx, |window, _| window.refresh());
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Anchored glass tip target: `(engine_id, window_id)` plus its hover rect.
type ProfileTipAnchor = Option<((String, String), HoverRect)>;

/// The real native window root and its application-thread entities.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent window, sidebar, and profile flags are tracked separately by the paint tree; packing them would conflate distinct render states"
)]
pub struct NativeApplication {
    /// A dismissible window-level error, such as a host that could not be
    /// added or a recent thread that could not be opened.
    window_error: Option<String>,
    machine_home: Option<std::path::PathBuf>,
    machine_label: String,
    machine_menu: impl_machines::MachineMenu,
    /// A switch away from this host that is draining its connection.
    host_switch: Option<host_switch::HostSwitchNotice>,
    theme: ArtisanTheme,
    desktop_theme: DesktopTheme,
    focus_handle: FocusHandle,
    #[cfg(test)]
    add_project_focus_handle: FocusHandle,
    service: Option<Arc<NativeTransportService>>,
    composer: Entity<NativeComposer>,
    run_controls: composer_run_controls::RunControlsState,
    composer_queue: composer_queue_application::QueueApplicationState,
    composer_controls: Entity<NativeComposerControls>,
    model_selector: Entity<NativeModelSelector>,
    deferred_composer_policy: Option<(ThreadId, crate::native_model_selector::SelectPolicy)>,
    composer_model_choice: Option<(
        Option<ThreadId>,
        crate::native_model_catalog::NativeModelPolicy,
    )>,
    /// The Forge's default engine configuration, shown on threads without
    /// their own until they save one.
    default_engine_config: Option<artisan_domain::EngineRunConfig>,
    /// Legacy file preferences handed to the Forge, removed once it answers.
    legacy_import: Option<crate::editor_settings::LegacyForgePreferences>,
    #[cfg(test)]
    test_legacy_preferences: Option<crate::editor_settings::LegacyForgePreferences>,
    composer_model_run_error: Option<String>,
    /// The selection whose Forge resolution is awaited; a later selection
    /// replaces it so only the latest answer is applied.
    pending_resolution: Option<(ThreadId, artisan_domain::CatalogSelection)>,
    catalog_controller: NativeCatalogController,
    host_model_catalog: Option<NativeModelCatalog>,
    connection_retry_pending: bool,
    _composer_controls_subscription: Subscription,
    _composer_model_subscription: Subscription,
    message_images: Entity<NativeMessageImages>,
    _message_images_subscription: Subscription,
    _composer_subscription: Subscription,
    _composer_observation: Subscription,
    profile_menu: DropdownMenuState,
    profile_focus: FocusHandle,
    profile_origin: Rc<Cell<Bounds<gpui::Pixels>>>,
    profile_name: Option<String>,
    profile_hostname: Option<String>,
    profile_usage: NativeProfileUsageState,
    engine_installs: impl_engine_installs::EngineInstallsState,
    profile_usage_generation: ProfileUsageGeneration,
    profile_usage_next_seq: u64,
    profile_hover: Rc<RefCell<SlidingHoverState>>,
    profile_hover_surface_bounds: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
    /// Whether the profile action pill currently follows keyboard
    /// navigation (`true`) or the pointer (`false`). GPUI can recompute
    /// hover during painting, so a surface-leave callback may arrive right
    /// after a keyboard move even though the pointer never left; the mode
    /// keeps that recomputation from discarding a keyboard-owned pill.
    profile_hover_keyboard: Cell<bool>,
    profile_usage_scroll: ScrollHandle,
    profile_usage_scroll_state: PickerScrollState,
    profile_usage_scroll_frame_scheduled: bool,
    /// Hovered usage meter as `(engine_id, window_id)`; the glass remaining
    /// tooltip follows this row until the pointer leaves, the wheel moves,
    /// or the menu closes.
    profile_meter_hover: Rc<RefCell<Option<(String, String)>>>,
    profile_tip_surface_bounds: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
    profile_tip_anchor: Rc<RefCell<ProfileTipAnchor>>,
    /// Stable keyboard focus per visible refresh control, pruned with the
    /// visible provider list so Enter/Space can refresh without new menu
    /// items.
    profile_refresh_focus: Rc<RefCell<Vec<(String, FocusHandle)>>>,
    /// Retained interruptible swap per refresh control, pruned with the
    /// visible provider list.
    profile_refresh_swap: Rc<RefCell<HashMap<String, RefreshSwap>>>,
    profile_swap_frame_scheduled: Rc<Cell<bool>>,
    profile_menu_motion: Rc<RefCell<PickerMenuMotion>>,
    profile_menu_motion_task: Option<Task<()>>,
    profile_tip_tween: Rc<RefCell<ProfileTipTween>>,
    command_menu: Entity<NativeCommandMenu>,
    _command_menu_observation: Subscription,
    sidebar_collapsed: bool,
    sidebar_navigation_focus: FocusHandle,
    sidebar_hover: Rc<RefCell<SlidingHoverState>>,
    sidebar_hover_surface_bounds: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
    message_flight: Option<NativeMessageFlight>,
    /// Keeps the connection open until the message flight's reply arrives.
    message_flight_hold: Option<crate::native_transport_service::Hold>,
    /// Forge draft save chains and uploads of this connection.
    composer_drafts: composer_drafts::ComposerDrafts,
    message_receipt: Option<QueueMessageReceipt>,
    message_failure: Option<NativeMessageFailure>,
    /// Refusal-specific banner copy (e.g. the starting-run guard). `None`
    /// renders the generic send-failure copy. Cleared with the failure.
    message_failure_note: Option<String>,
    picker: Option<Entity<ProjectPickerView>>,
    picker_subscription: Option<Subscription>,
    home_picker: Option<Entity<HomeProjectPickerView>>,
    home_picker_subscription: Option<Subscription>,
    project_navigation: impl_projects::ProjectNavigation,
    project_options: Vec<ProjectOption>,
    selected_project: Option<ProjectId>,
    /// Retained repository facts for the titlebar workspace header.
    ///
    /// `None` paints the project-folder fallback. The facts come from the
    /// bounded `QueryProjectRepository` read of the selected project's stored
    /// root and are never synthesized; the titlebar renders real inspected
    /// repository data or nothing.
    titlebar_repository: Option<TitlebarRepository>,
    /// The project whose repository facts are retained (or requested).
    ///
    /// Fences stale replies: a repository observation for a project that is
    /// no longer selected cannot replace the current header.
    titlebar_repository_project: Option<ProjectId>,
    /// The latest authoritative thread listing for `selected_project`.
    thread_listing: Option<ThreadListing>,
    sidebar_threads: impl_sidebar_threads::SidebarThreadsState,
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
    /// Paired engine observation rows for the selected thread.
    ///
    /// Fed by uni-stream observation events keyed to the selected thread.
    /// Cursor ordering and reconnect dedup live in the state itself, which is
    /// independent of host mounting; the state resets when the selected
    /// thread changes.
    engine_observations: Option<EngineObservationState>,
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
    settings_screen_subscription: Option<Subscription>,
    intake_stage: Option<NativeProjectIntakeStage>,
    intake_failure_operation: Option<NativeProjectIntakeOperation>,
    intake_retry_available: bool,
    intake_restore_state: Option<NativeViewState>,
    /// The running intake opens a thread the Forge created with its own
    /// draft (a recovered failed prompt), so the composer's draft does not
    /// carry into it.
    intake_opens_forge_draft: bool,
    service_stopped: bool,
    shutdown_prepared: bool,
    #[cfg(test)]
    test_command_sink: Option<NativeTestCommandSink>,
    poll_task: Option<Task<()>>,
    engine_settings: EngineSettingsController,
}

#[cfg(test)]
#[path = "native_application/tests.rs"]
mod tests;

#[path = "native_composer_run_controls.rs"]
mod composer_run_controls;

#[path = "native_composer_models.rs"]
mod native_composer_models;

#[path = "native_composer_queue_application.rs"]
mod composer_queue_application;

#[path = "native_application/impl_projects.rs"]
mod impl_projects;
#[path = "native_application/impl_recent_threads.rs"]
mod impl_recent_threads;
#[path = "native_application/impl_sidebar_threads.rs"]
mod impl_sidebar_threads;

mod draft_send;
mod forge_outbox;
mod host_state;
mod preferences;
