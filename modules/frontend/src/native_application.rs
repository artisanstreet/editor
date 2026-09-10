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
use artisan_protocol::{ConversationSubscriptionStarted, QueueMessageReceipt, ServerEvent};
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::button::{
    AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility,
};
use artisan_ui::card::{CardStyle, compact_card, compact_card_content};
use artisan_ui::dropdown_menu::{DropdownMenuEntry, DropdownMenuItem, DropdownMenuState};
use artisan_ui::fade_arc::FadeArc;
use artisan_ui::icon::{IconSize, IconStyle, IconTint, icon};
use artisan_ui::motion::{MotionCurve, MotionDuration, MotionPolicy};
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::{ArtisanTheme, DesktopTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::prelude::FluentBuilder as _;
use gpui::Focusable as _;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, ClickEvent, ClipboardItem, Context, Div, Entity,
    FocusHandle, FontWeight, HighlightStyle, KeyBinding, Render, ScrollHandle, ScrollWheelEvent,
    SharedString, Stateful, StatefulInteractiveElement, StyledImage as _, StyledText, Subscription,
    Task, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, canvas, div,
    prelude::{InteractiveElement as _, IntoElement, ParentElement as _, Styled as _},
    px, size,
};

use crate::composer::{DraftDisposition, SubmissionBlocked, SubmissionToken};
use crate::desktop_shell::{
    DESKTOP_COMPOSER_SELECTOR, DESKTOP_HOME_SELECTOR, desktop_muted, desktop_nav_glyph,
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
    EngineReadiness, NativeProfileUsageState, NativeUsageEntry, NativeUsageWindow,
    ProfileUsageGeneration, account_usage_response_current, catalog_with_usage_readiness,
    checked_label, engine_readiness, engine_refresh_failure, group_usage_windows,
    plan_profile_usage_loads, profile_usage_display_name, reset_duration, tip_run_up_from,
    usage_remaining_percent,
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
    CommandSendError, EventReceiveError, NativeProjectIntakeOperation, NativeProjectIntakeStage,
    NativeTransportCommand, NativeTransportEvent, NativeTransportService, ServiceFailure,
    ServiceFailureCategory, ServiceFailureStage, ServiceStopStatus, SettingsLoadGeneration,
};
use crate::onboarding_harness_presentation::{
    HarnessCatalog, HarnessSetupAction, HarnessSetupState,
};
use crate::onboarding_screen::{OnboardingHarnessEntry, OnboardingScreen};
use crate::thread_environment_presentation::{HostIdentitySnapshot, ThreadEnvironmentInput};
use crate::thread_screen::{ThreadScreen, ThreadScreenGate};
use crate::usage_meter::usage_segment_fraction;
use crate::workspace_tab_state::EditorViewState;
use crate::{
    conversation_delivery_machine::{ConversationDeliveryEffect, ConversationDeliveryEvent},
    conversation_host::{CONVERSATION_HOST_MAX_EFFECTS, ConversationHost, ConversationHostEffect},
    conversation_state_machine::{ConversationStateEffect, ConversationStateEvent},
    conversation_view_machine::ViewportState,
    engine_observation_state::{ApplyOutcome, EngineObservationState},
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
const SIDEBAR_NEW_THREAD_HOVER_ID: &str = "new-thread";
const SIDEBAR_MARKETPLACE_HOVER_ID: &str = "marketplace";
const SIDEBAR_PROFILE_HOVER_ID: &str = "profile";

/// Probe measuring one sidebar row against the shared sidebar hover
/// surface, so New thread, Marketplace, and the profile footer all drive
/// the same sliding pill from their actual bounds.
fn sidebar_hover_probe(
    hover: Rc<RefCell<SlidingHoverState>>,
    surface_bounds: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
    id: &'static str,
) -> gpui::Canvas<()> {
    let measured_id = id.to_owned();
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
const PROFILE_SETTINGS_HOVER_ID: &str = "profile-settings";
const PROFILE_USAGE_HOVER_ID: &str = "profile-usage";
/// Breathing room kept between the profile panel top edge and the viewport.
const PROFILE_MENU_VIEWPORT_MARGIN_PX: f32 = 8.0;
/// Vertical gap between the panel bottom and the profile trigger top,
/// matching the source content `sideOffset` and the anchored offset applied
/// when placing the panel.
const PROFILE_MENU_ANCHOR_GAP_PX: f32 = 4.0;
/// Swap target for one refresh control: the reading at rest, the action
/// on hover or keyboard focus, the spinner while its refresh is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefreshSwapTarget {
    Reading,
    Action,
    Loading,
}

impl RefreshSwapTarget {
    /// Displayed endpoints per reading in [reading, action, spinner] order.
    fn values(self) -> [f32; 3] {
        match self {
            Self::Reading => [1.0, 0.0, 0.0],
            Self::Action => [0.0, 1.0, 0.0],
            Self::Loading => [0.0, 0.0, 1.0],
        }
    }
}

/// Retained interruptible swap for one refresh control. Retargets always
/// start from the currently displayed values, so rapid hover/focus/refresh
/// changes reverse mid-flight exactly like the source transition. Opacity,
/// blur, and paint offset are each retained and interpolated, so a reversal
/// can never flip an offset sign mid-flight.
#[derive(Clone, Copy, Debug)]
struct RefreshSwap {
    from: [f32; 3],
    displayed: [f32; 3],
    to: [f32; 3],
    off_from: [f32; 3],
    off_displayed: [f32; 3],
    off_to: [f32; 3],
    started_ms: i64,
    hovered: bool,
}

impl RefreshSwap {
    fn resting() -> Self {
        Self {
            from: [1.0, 0.0, 0.0],
            displayed: [1.0, 0.0, 0.0],
            to: [1.0, 0.0, 0.0],
            off_from: [0.0, 4.0, 4.0],
            off_displayed: [0.0, 4.0, 4.0],
            off_to: [0.0, 4.0, 4.0],
            started_ms: 0,
            hovered: false,
        }
    }
}

/// Paint offsets per target: shown readings sit at zero; the hidden
/// reading exits upward while action and spinner rest below, matching the
/// source hidden frames. A reversal keeps interpolating its retained
/// offset, so the sign can never flip mid-flight.
fn swap_offsets_for(to: [f32; 3]) -> [f32; 3] {
    [
        if to[0] >= 1.0 { 0.0 } else { -4.0 },
        if to[1] >= 1.0 { 0.0 } else { 4.0 },
        if to[2] >= 1.0 { 0.0 } else { 4.0 },
    ]
}
/// Width of one meter tick: fourteen full 72/14 pitches with a 2px
/// transparent tail inside every pitch including the last.
const PROFILE_METER_TICK_PX: f32 = 72.0 / 14.0 - 2.0;
/// Shared remaining-value tween for meter tooltips: the first reading of
/// a menu-open session runs up from just short of its value, later rows
/// carry the displayed value across, all on the source 250ms smooth-out
/// curve (`MotionDuration::Fast` + `MotionCurve::SmoothOut`, matching
/// `--duration-fast` and `--ease-smooth-out`).
#[derive(Clone, Copy, Debug)]
struct ProfileTipTween {
    displayed: f64,
    from: f64,
    to: f64,
    started_ms: i64,
    seen: bool,
    scheduled: bool,
}

impl Default for ProfileTipTween {
    fn default() -> Self {
        Self {
            displayed: 0.0,
            from: 0.0,
            to: 0.0,
            started_ms: 0,
            seen: false,
            scheduled: false,
        }
    }
}
/// Fixed vertical chrome inside the profile panel: header (avatar 32 +
/// vertical padding 32), two separators (1 + margins 8 each), and the action
/// section (container padding 8 + two 36px rows). The usage area scrolls
/// above this chrome under the viewport cap.
const PROFILE_MENU_FIXED_CHROME_PX: f32 = 68.0 + 18.0 + 80.0;

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

/// A first send held while its engine-configuration save is admitted.
///
/// The composer flight is already begun (so the draft is reserved and later
/// sends are fenced), but no transport command is issued until the
/// authoritative save acknowledgment arrives. The pending send is scoped to
/// its thread and exact payload: a thread switch, a draft change, or a save
/// failure suppresses it without queueing anything. This type owns message
/// text and therefore implements neither `Debug` nor `Display`.
struct PendingFirstSend {
    thread_id: ThreadId,
    payload: QueueMessagePayload,
    token: SubmissionToken,
}

/// Outcome of first-send admission for a thread without a persisted engine
/// configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FirstSendAdmission {
    /// The thread is already configured; continue the send.
    Proceed,
    /// The send was held for a save, adopted, blocked, or suppressed; the
    /// gate already synced and notified.
    Held,
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
    composer_model_choice: Option<(
        Option<ThreadId>,
        crate::native_model_catalog::NativeModelPolicy,
    )>,
    composer_model_run_error: Option<String>,
    pending_first_send: Option<PendingFirstSend>,
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
    profile_usage: NativeProfileUsageState,
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
    profile_tip_anchor: Rc<RefCell<Option<((String, String), HoverRect)>>>,
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
    message_retry: Option<NativeMessageRetry>,
    message_receipt: Option<QueueMessageReceipt>,
    message_failure: Option<NativeMessageFailure>,
    picker: Option<Entity<ProjectPickerView>>,
    picker_subscription: Option<Subscription>,
    home_picker: Option<Entity<HomeProjectPickerView>>,
    home_picker_subscription: Option<Subscription>,
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
            pending_first_send: None,
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
            picker: None,
            picker_subscription: None,
            home_picker: None,
            home_picker_subscription: None,
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
    ///
    /// Entering Settings also requests fresh provider-account reads, so the
    /// engine pages observe true readiness instead of a stale row; the
    /// profile popover keeps its own open-time refresh.
    pub fn navigate(&mut self, route: NativeRoute, cx: &mut Context<Self>) {
        if matches!(route, NativeRoute::Settings { .. }) {
            self.ensure_profile_usage(false, None, cx);
        }
        self.route_history.navigate(route);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    /// Returns to the previous route when history exists, then rerenders.
    pub fn go_back(&mut self, cx: &mut Context<Self>) -> bool {
        let moved = self.route_history.go_back();
        if moved {
            self.sync_composer_availability(cx);
            cx.notify();
        }
        moved
    }

    /// Renders the home headline surface: a muted emblem, the centered
    /// "What should we build…?" heading with the inline project switcher,
    /// and nothing else — the editable composer lives in the shell footer.
    /// The Failure branch keeps the actionable offline error; every other
    /// branch drops the old "Start a task" copy and subtitle.
    fn new_thread_surface_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(24.0))
            .pb(px(24.0))
            .debug_selector(|| DESKTOP_HOME_SELECTOR.to_string())
            .child(self.home_emblem());
        if matches!(&self.state, NativeViewState::Failure(_)) {
            return root
                .child(self.home_heading("Forge is offline"))
                .child(
                    div()
                        .mt(px(8.0))
                        .max_w(px(440.0))
                        .text_size(px(15.0))
                        .text_color(self.desktop_theme.secondary)
                        .child(
                            "Forge is offline. Existing project and task data will remain visible when it reconnects."
                                .to_owned(),
                        ),
                );
        }
        match self.selected_project_name() {
            Some(name) => root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .child(self.home_heading("What should we build in\u{a0}"))
                    .child(self.home_project_trigger(&name, window, cx))
                    .child(self.home_heading("?")),
            ),
            None => root
                .child(self.home_heading("What should we build?"))
                .child(self.home_project_trigger(HOME_CHOOSE_PROJECT_LABEL, window, cx)),
        }
    }

    /// Centered large regular home heading line.
    fn home_heading(&self, text: &str) -> Div {
        div()
            .text_size(px(HOME_HEADLINE_TEXT_PX))
            .font_weight(FontWeight::NORMAL)
            .text_color(self.desktop_theme.foreground)
            .child(text.to_owned())
    }

    /// Small muted emblem above the home heading.
    fn home_emblem(&self) -> Div {
        div().mb(px(16.0)).child(
            asset_glyph(AssetId::TABLER_TERMINAL_2)
                .size(px(HOME_EMBLEM_SIZE_PX))
                .text_color(self.desktop_theme.secondary),
        )
    }

    /// The inline project switcher for the home heading: the live picker
    /// when installed, otherwise the static label (before the first project
    /// listing arrives). Text styling inherits from the heading container.
    fn home_project_trigger(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(home) = self.home_picker.clone() else {
            return div()
                .text_size(px(HOME_HEADLINE_TEXT_PX))
                .font_weight(FontWeight::NORMAL)
                .text_color(self.desktop_theme.foreground)
                .child(label.to_owned())
                .into_any_element();
        };
        home.update(cx, |picker, picker_cx| {
            picker.render_inline_trigger(label, window, picker_cx)
        })
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

    /// Native titlebar identity: the `Artisan Editor` wordmark. The
    /// wordmark keeps the home navigation.
    fn desktop_identity(&self, cx: &Context<Self>) -> Div {
        div()
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
                    .text_size(px(20.0))
                    .font_family("Artisan Neo")
                    .font_weight(FontWeight::SEMIBOLD)
                    // -0.05em tracking at 20px: 20 * -0.05 = -1.0px.
                    .letter_spacing(px(-1.0))
                    .text_color(self.desktop_theme.foreground)
                    .child("Artisan Editor"),
            )
    }

    fn desktop_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = self.desktop_theme;
        let sidebar_item_radius = px(6.0);
        let visible_hover_ids = vec![
            SIDEBAR_NEW_THREAD_HOVER_ID.to_owned(),
            SIDEBAR_MARKETPLACE_HOVER_ID.to_owned(),
            SIDEBAR_PROFILE_HOVER_ID.to_owned(),
        ];
        self.sidebar_hover
            .borrow_mut()
            .clear_if_missing(&visible_hover_ids);

        let sidebar_hover = Rc::clone(&self.sidebar_hover);
        let sidebar_hover_surface_bounds = Rc::clone(&self.sidebar_hover_surface_bounds);
        let surface_bounds = Rc::clone(&sidebar_hover_surface_bounds);
        let surface_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let changed = {
                    let mut surface = surface_bounds.borrow_mut();
                    if *surface == Some(bounds) {
                        false
                    } else {
                        *surface = Some(bounds);
                        true
                    }
                };
                if changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let mut nav_theme = theme;
        nav_theme.secondary = self.theme.colors.muted_foreground.to_paint();
        let nav = div()
            .id("artisan-workspace-navigation")
            .relative()
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
            .debug_selector(|| "artisan-workspace-navigation".to_owned())
            .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                if *hovered {
                    app.sidebar_hover
                        .borrow_mut()
                        .set_active(SIDEBAR_NEW_THREAD_HOVER_ID.to_owned());
                } else if app.sidebar_hover.borrow().active_id() == Some(SIDEBAR_NEW_THREAD_HOVER_ID) {
                    // Hide, don't clear: the retained rect keeps the next
                    // row-to-row flight sliding instead of snapping.
                    app.sidebar_hover.borrow_mut().hide();
                }
                cx.notify();
            }))
            .child(sidebar_hover_probe(
                Rc::clone(&sidebar_hover),
                Rc::clone(&sidebar_hover_surface_bounds),
                SIDEBAR_NEW_THREAD_HOVER_ID,
            ))
            .child(desktop_nav_glyph(AssetId::TABLER_EDIT, nav_theme))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.foreground)
                    .child("New thread"),
            )
            .on_click(cx.listener(|app, _, window, cx| {
                window.focus(&app.sidebar_navigation_focus, cx);
                app.begin_new_task(cx);
            }))
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    app.begin_new_task(cx);
                }
            }));
        let marketplace = div()
            .id("artisan-marketplace-navigation")
            .w_full()
            .h(px(34.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .relative()
            .debug_selector(|| "artisan-marketplace-navigation".to_owned())
            .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                if *hovered {
                    app.sidebar_hover
                        .borrow_mut()
                        .set_active(SIDEBAR_MARKETPLACE_HOVER_ID.to_owned());
                } else if app.sidebar_hover.borrow().active_id() == Some(SIDEBAR_MARKETPLACE_HOVER_ID) {
                    app.sidebar_hover.borrow_mut().hide();
                }
                cx.notify();
            }))
            .child(sidebar_hover_probe(
                Rc::clone(&sidebar_hover),
                Rc::clone(&sidebar_hover_surface_bounds),
                SIDEBAR_MARKETPLACE_HOVER_ID,
            ))
            .child(desktop_nav_glyph(AssetId::TABLER_SHOPPING_BAG, nav_theme))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.foreground)
                    .child("Marketplace"),
            );
        // One shared hover surface for the whole sidebar column: the same
        // sliding pill travels among New thread, Marketplace, and the
        // profile footer, measured against these bounds. Rows hide the pill
        // on departure and the spacer hides it on entry, all retaining
        // geometry so row-to-row keeps sliding; leaving the column clears
        // it as well.
        let navigation = div()
            .id("artisan-workspace-navigation-hover-surface")
            .relative()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                if !*hovered {
                    app.sidebar_hover.borrow_mut().clear();
                    cx.notify();
                }
            }))
            .child(surface_probe)
            .child(render_picker_hover_pill(
                self.theme,
                Rc::clone(&sidebar_hover),
                "sidebar",
                sidebar_item_radius,
                cx.reduce_motion(),
            ))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(nav)
                    .child(marketplace),
            )
            .child(
                div()
                    .id("artisan-sidebar-spacer")
                    .flex_1()
                    .min_h(px(0.0))
                    .debug_selector(|| "artisan-sidebar-spacer".to_owned())
                    .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                        if *hovered {
                            app.sidebar_hover.borrow_mut().hide();
                            cx.notify();
                        }
                    })),
            )
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .h(px(1.0))
                            .mx(px(-10.0))
                            .bg(theme.line)
                            .debug_selector(|| "artisan-sidebar-footer-divider".to_owned()),
                    )
                    .child(self.desktop_profile(window, cx)),
            );
        div()
            .h_full()
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(10.0))
            .child(navigation)
    }

    fn profile_hover_id_for_index(index: usize) -> Option<String> {
        match index {
            0 => Some(PROFILE_SETTINGS_HOVER_ID.to_owned()),
            1 => Some(PROFILE_USAGE_HOVER_ID.to_owned()),
            _ => None,
        }
    }

    fn set_profile_highlight(&mut self, index: usize) {
        if index == 0 {
            let _ = self.profile_menu.move_first();
        } else {
            let _ = self.profile_menu.move_last();
        }
    }

    fn sync_profile_hover_to_highlight(&self) {
        if let Some(index) = self.profile_menu.highlighted_index()
            && let Some(id) = Self::profile_hover_id_for_index(index)
        {
            self.profile_hover.borrow_mut().set_active(id);
            self.profile_hover_keyboard.set(true);
        }
    }

    /// Returns the stable keyboard focus for one refresh control,
    /// creating it on first paint. Handles are pruned with the visible
    /// provider list in [`Self::desktop_profile_usage`].
    fn profile_refresh_focus_handle(&self, engine_id: &str, cx: &Context<Self>) -> FocusHandle {
        if let Some(handle) = self
            .profile_refresh_focus
            .borrow()
            .iter()
            .find(|(id, _)| id == engine_id)
            .map(|(_, handle)| handle.clone())
        {
            return handle;
        }
        let handle = cx.focus_handle();
        self.profile_refresh_focus
            .borrow_mut()
            .push((engine_id.to_owned(), handle.clone()));
        handle
    }

    /// Source `bg-border/50` for dropdown separators: the shared border
    /// token with its original alpha multiplied by one half rather than
    /// overridden, so an already-translucent border stays proportional.
    fn profile_separator_paint(&self) -> gpui::Hsla {
        let border = self.theme.colors.border;
        border.with_alpha(border.a * 0.5).to_paint()
    }

    fn clear_profile_hover(&self) {
        if self.profile_hover.borrow().visible() {
            self.profile_hover.borrow_mut().clear();
        }
        self.profile_hover_surface_bounds.borrow_mut().take();
        self.profile_hover_keyboard.set(false);
        self.profile_meter_hover.borrow_mut().take();
        self.profile_tip_surface_bounds.borrow_mut().take();
        self.profile_tip_anchor.borrow_mut().take();
        *self.profile_tip_tween.borrow_mut() = ProfileTipTween::default();
        self.profile_refresh_swap.borrow_mut().clear();
        self.profile_swap_frame_scheduled.set(false);
    }

    /// Maximum height for the scrollable usage area: the natural content
    /// height wins until the panel would outgrow the space above the
    /// trigger, keeping a small viewport margin. The fixed header, divider,
    /// action rows, and panel padding are subtracted so only the usage
    /// area scrolls. Short windows clamp at zero instead of overflowing.
    fn profile_usage_max_height(&self, window: &Window) -> gpui::Pixels {
        let viewport_height = f32::from(window.bounds().size.height);
        let trigger_top = f32::from(self.profile_origin.get().top()).min(viewport_height);
        let available = trigger_top - PROFILE_MENU_ANCHOR_GAP_PX - PROFILE_MENU_VIEWPORT_MARGIN_PX;
        px((available - PROFILE_MENU_FIXED_CHROME_PX).max(0.0))
    }

    fn handle_profile_usage_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Same contract as the model picker: this wrapper is the first
        // bubble listener inside the scroll container, so consuming the
        // wheel here keeps GPUI from applying its default offset update a
        // second time.
        cx.stop_propagation();
        if !self.profile_menu_is_interactive() {
            return;
        }
        // A scrolling list must not keep a meter tooltip pinned to a stale
        // row geometry.
        self.profile_meter_hover.borrow_mut().take();
        self.profile_tip_anchor.borrow_mut().take();
        let delta = event.delta.pixel_delta(window.line_height()).y;
        let delta = f32::from(delta);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        let handle = self.profile_usage_scroll.clone();
        let offset = handle.offset();
        let current = f32::from(offset.y);
        let maximum = f32::from(handle.max_offset().y).max(0.0);
        if event.delta.precise() || cx.reduce_motion() {
            let next = (current + delta).clamp(-maximum, 0.0);
            handle.set_offset(gpui::point(offset.x, px(next)));
            self.profile_usage_scroll_state.cancel_to(next, maximum);
            cx.notify();
            return;
        }
        self.profile_usage_scroll_state
            .push(current, delta, maximum);
        self.schedule_profile_usage_scroll_frame(window, cx);
    }

    fn schedule_profile_usage_scroll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.profile_usage_scroll_state.active() || self.profile_usage_scroll_frame_scheduled {
            return;
        }
        self.profile_usage_scroll_frame_scheduled = true;
        cx.on_next_frame(window, |application, window, cx| {
            application.advance_profile_usage_scroll(window, cx);
        });
    }

    fn advance_profile_usage_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_usage_scroll_frame_scheduled = false;
        let offset = self.profile_usage_scroll.offset();
        let maximum = f32::from(self.profile_usage_scroll.max_offset().y).max(0.0);
        if let Some(next) = self
            .profile_usage_scroll_state
            .step(f32::from(offset.y), maximum)
        {
            self.profile_usage_scroll
                .set_offset(gpui::point(offset.x, px(next)));
            cx.notify();
        }
        self.schedule_profile_usage_scroll_frame(window, cx);
    }

    /// Settles a queued scroll target at the current offset so dismissing
    /// the menu cannot leave inertia pending for the next open.
    fn cancel_profile_usage_scroll(&mut self) {
        let offset = self.profile_usage_scroll.offset();
        let maximum = f32::from(self.profile_usage_scroll.max_offset().y).max(0.0);
        self.profile_usage_scroll_state
            .cancel_to(f32::from(offset.y), maximum);
        self.profile_usage_scroll_frame_scheduled = false;
    }

    /// Whether the profile menu accepts pointer and wheel input: open and
    /// past its retained exit presentation, mirroring the model picker.
    fn profile_menu_is_interactive(&self) -> bool {
        self.profile_menu.is_open()
            && self.profile_menu_motion.borrow().phase() != PickerMenuPhase::Closing
    }

    fn begin_profile_menu_open(&mut self, cx: &mut Context<Self>) {
        let generation = self.profile_menu_motion.borrow_mut().begin_open();
        if cx.reduce_motion() {
            self.profile_menu_motion
                .borrow_mut()
                .finish_open(generation);
            self.profile_menu_motion_task = None;
        } else {
            self.schedule_profile_menu_motion_settle(generation, true, cx);
        }
    }

    fn begin_profile_menu_close(&mut self, cx: &mut Context<Self>) {
        let generation = self.profile_menu_motion.borrow_mut().begin_close();
        if cx.reduce_motion() {
            self.profile_menu_motion
                .borrow_mut()
                .finish_close(generation);
            self.profile_menu_motion_task = None;
        } else {
            self.schedule_profile_menu_motion_settle(generation, false, cx);
        }
    }

    fn schedule_profile_menu_motion_settle(
        &mut self,
        generation: u64,
        opening: bool,
        cx: &mut Context<Self>,
    ) {
        self.profile_menu_motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PICKER_MENU_MOTION_DURATION_MS))
                .await;
            let _ = this.update(cx, |application, cx| {
                let settled = if opening {
                    application
                        .profile_menu_motion
                        .borrow_mut()
                        .finish_open(generation)
                } else {
                    application
                        .profile_menu_motion
                        .borrow_mut()
                        .finish_close(generation)
                };
                if settled {
                    application.profile_menu_motion_task = None;
                    cx.notify();
                }
            });
        }));
    }

    fn schedule_profile_tip_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settled = {
            let tween = self.profile_tip_tween.borrow();
            tween.displayed == tween.to
        };
        if settled || self.profile_tip_tween.borrow().scheduled {
            return;
        }
        self.profile_tip_tween.borrow_mut().scheduled = true;
        cx.on_next_frame(window, |application, window, cx| {
            application.advance_profile_tip(window, cx);
        });
    }

    fn advance_profile_tip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_tip_tween.borrow_mut().scheduled = false;
        let total_ms = MotionDuration::Fast.as_duration().as_millis() as f64;
        let now_ms = profile_usage_now_ms();
        let done = {
            let mut tween = self.profile_tip_tween.borrow_mut();
            let elapsed = now_ms.saturating_sub(tween.started_ms).max(0) as f64;
            let progress = (elapsed / total_ms).clamp(0.0, 1.0);
            let eased = MotionCurve::SmoothOut.sample(progress);
            tween.displayed = tween.from + (tween.to - tween.from) * eased;
            if progress >= 1.0 {
                tween.displayed = tween.to;
                true
            } else {
                false
            }
        };
        cx.notify();
        if !done {
            self.schedule_profile_tip_frame(window, cx);
        }
    }

    /// Moves one refresh control toward its target state, starting from the
    /// currently displayed values so rapid hover/focus/refresh changes
    /// reverse mid-flight. Reduced motion settles instantly.
    fn retarget_profile_swap(
        &self,
        engine_id: &str,
        target: RefreshSwapTarget,
        reduce_motion: bool,
    ) {
        let mut swaps = self.profile_refresh_swap.borrow_mut();
        let swap = swaps
            .entry(engine_id.to_owned())
            .or_insert_with(RefreshSwap::resting);
        let to = target.values();
        // A reduced-motion change settles everything immediately even when
        // the target itself is unchanged, so already-queued steps observe
        // settled endpoints instead of regressing toward stale ones.
        if reduce_motion {
            let off = swap_offsets_for(to);
            swap.from = to;
            swap.displayed = to;
            swap.to = to;
            swap.off_from = off;
            swap.off_displayed = off;
            swap.off_to = off;
            return;
        }
        if swap.to == to {
            return;
        }
        swap.from = swap.displayed;
        swap.to = to;
        swap.off_from = swap.off_displayed;
        swap.off_to = swap_offsets_for(to);
        swap.started_ms = profile_usage_now_ms();
    }

    /// Recomputes one control's target from its live hover/focus/refresh
    /// inputs and ensures the frame driver runs while anything is moving.
    /// Called from pointer events and every render so keyboard focus changes
    /// that arrive without pointer events still animate.
    fn refresh_profile_swap(
        &self,
        engine_id: &str,
        refreshing: bool,
        reduce_motion: bool,
        window: &mut Window,
        cx: &Context<Self>,
    ) {
        let hovered = self
            .profile_refresh_swap
            .borrow()
            .get(engine_id)
            .is_some_and(|swap| swap.hovered);
        let focused = self
            .profile_refresh_focus
            .borrow()
            .iter()
            .find(|(id, _)| id == engine_id)
            .is_some_and(|(_, handle)| handle.is_focused(window));
        let target = if refreshing {
            RefreshSwapTarget::Loading
        } else if hovered || focused {
            RefreshSwapTarget::Action
        } else {
            RefreshSwapTarget::Reading
        };
        self.retarget_profile_swap(engine_id, target, reduce_motion);
        self.schedule_profile_swap_frame(window, cx);
    }

    fn schedule_profile_swap_frame(&self, window: &mut Window, cx: &Context<Self>) {
        let pending = self
            .profile_refresh_swap
            .borrow()
            .values()
            .any(|swap| swap.displayed != swap.to);
        if !pending || self.profile_swap_frame_scheduled.get() {
            return;
        }
        self.profile_swap_frame_scheduled.set(true);
        cx.on_next_frame(window, |application, window, cx| {
            application.advance_profile_swap(window, cx);
        });
    }

    fn advance_profile_swap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_swap_frame_scheduled.set(false);
        let running = self.step_profile_swaps(profile_usage_now_ms());
        cx.notify();
        if running {
            self.schedule_profile_swap_frame(window, cx);
        }
    }

    /// Steps every retained swap toward its target on the source 150ms
    /// ease-in-out curve (`MotionDuration::Quick` + `MotionCurve::EaseInOut`,
    /// matching `--text-swap-dur` and `ease-in-out`). Returns whether any
    /// swap is still moving. Pure over the passed clock so tests can drive
    /// interrupted transitions deterministically.
    fn step_profile_swaps(&self, now_ms: i64) -> bool {
        let total_ms = MotionDuration::Quick.as_duration().as_millis() as f64;
        let mut running = false;
        let mut swaps = self.profile_refresh_swap.borrow_mut();
        for swap in swaps.values_mut() {
            // Fully settled entries are never re-driven: their clock may be
            // newer than their values after a reduced-motion settle.
            if swap.displayed == swap.to && swap.off_displayed == swap.off_to {
                continue;
            }
            let elapsed = now_ms.saturating_sub(swap.started_ms).max(0) as f64;
            let progress = (elapsed / total_ms).clamp(0.0, 1.0);
            let eased = MotionCurve::EaseInOut.sample(progress) as f32;
            for index in 0..3 {
                swap.displayed[index] =
                    swap.from[index] + (swap.to[index] - swap.from[index]) * eased;
                swap.off_displayed[index] =
                    swap.off_from[index] + (swap.off_to[index] - swap.off_from[index]) * eased;
            }
            if progress >= 1.0 {
                swap.displayed = swap.to;
                swap.off_displayed = swap.off_to;
            } else {
                running = true;
            }
        }
        running
    }

    fn activate_profile_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for action in self.profile_menu.take_actions() {
            match action.item_id().as_ref() {
                "usage" => {
                    // The Usage action forces a refresh and keeps the menu
                    // open. Provider rows are never invented locally; the
                    // native adapter owns refresh and provider data.
                    // Restore the Usage highlight so the sliding pill stays
                    // on the row under the pointer instead of jumping to
                    // Settings after the transient state clears.
                    self.profile_menu.set_open(true);
                    self.set_profile_highlight(1);
                    self.profile_hover
                        .borrow_mut()
                        .set_active(PROFILE_USAGE_HOVER_ID.to_owned());
                    self.profile_hover_keyboard.set(false);
                    self.ensure_profile_usage(true, None, cx);
                }
                "settings" => {
                    self.clear_profile_hover();
                    self.cancel_profile_usage_scroll();
                    self.begin_profile_menu_close(cx);
                    self.navigate(
                        NativeRoute::Settings {
                            section: SettingsRoute::Models,
                            engine: None,
                        },
                        cx,
                    );
                }
                _ => {}
            }
        }
        window.focus(&self.profile_focus, cx);
        cx.notify();
    }

    fn desktop_profile_usage(
        &self,
        theme: DesktopTheme,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let section = div()
            .id(crate::native_profile_usage::PROFILE_USAGE_SELECTOR)
            .debug_selector(|| crate::native_profile_usage::PROFILE_USAGE_SELECTOR.to_owned())
            .flex()
            .flex_col();

        if !self.profile_usage_connected() {
            return section
                .gap(px(3.0))
                .px(px(8.0))
                .py(px(6.0))
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(theme.foreground)
                        .child("Usage"),
                )
                .child(
                    desktop_muted(theme, "Connect to Forge to see usage.")
                        .line_height(px(14.0))
                        .text_size(px(11.0)),
                );
        }

        let visible = self.profile_usage.visible_usage_entries();
        self.profile_refresh_focus.borrow_mut().retain(|(id, _)| {
            visible
                .iter()
                .any(|entry| entry.engine_id.as_str() == id.as_str())
        });
        self.profile_refresh_swap.borrow_mut().retain(|id, _| {
            visible
                .iter()
                .any(|entry| entry.engine_id.as_str() == id.as_str())
        });
        if visible.is_empty() {
            return section.child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(theme.secondary)
                    .child("No engine accounts connected."),
            );
        }

        let now_ms = profile_usage_now_ms();
        let mut section = section.px(px(4.0)).py(px(4.0));
        for (index, entry) in visible.iter().enumerate() {
            if index > 0 {
                section = section.child(
                    div()
                        .h(px(1.0))
                        .bg(self.profile_separator_paint())
                        .my(px(4.0)),
                );
            }
            section =
                section.child(self.desktop_profile_usage_engine(entry, theme, window, now_ms, cx));
        }
        section
    }

    /// One provider block matching `sidebar-engine-usage.svelte`: the engine
    /// mark plus name with an inline hover-swapping refresh control, one
    /// cadence group per disclosed cadence with 12px labels and 72x8 accent
    /// meters, and a muted reset sentence with the duration in foreground.
    /// Entries without renderable windows never reach this renderer (see
    /// [`NativeProfileUsageState::visible_usage_entries`]).
    fn desktop_profile_usage_engine(
        &self,
        entry: &NativeUsageEntry,
        theme: DesktopTheme,
        window: &mut Window,
        now_ms: i64,
        cx: &Context<Self>,
    ) -> Div {
        let engine_id = entry.engine_id.clone();
        let Some(report) = entry.report.as_ref() else {
            return div();
        };
        let refreshing = self
            .profile_usage
            .refreshing_engine_ids
            .iter()
            .any(|current| current == &engine_id);
        let mark_asset = engine_asset(&engine_id);
        let accent = engine_accent(&engine_id)
            .map(|hex| gpui::rgb_to_hsla(gpui::rgb(hex)))
            .unwrap_or_else(|| self.theme.colors.primary.to_paint());
        let dim = self.theme.colors.foreground.with_alpha(0.11).to_paint();
        let block_selector = format!("artisan-profile-usage-engine-{engine_id}");
        let mut block = div()
            .debug_selector({
                let block_selector = block_selector.clone();
                move || block_selector.clone()
            })
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0));
        let mut title = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .min_w(px(0.0))
                    .items_center()
                    .gap(px(8.0))
                    // Foreground ambient: monochrome provider marks resolve
                    // through it like source `dark:invert`; full-color marks
                    // keep their authored colors either way.
                    .text_color(theme.foreground)
                    .child(
                        icon(IconStyle::resolve(
                            self.theme,
                            mark_asset,
                            IconSize::Default,
                            if mark_asset == AssetId::TABLER_QUESTION_MARK {
                                IconTint::Muted
                            } else {
                                IconTint::Inherit
                            },
                        ))
                        .size(px(16.0))
                        .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(report.display_name.clone()),
                    ),
            );
        if let Some(checked) = checked_label(entry.fetched_at_ms, now_ms) {
            title = title.child(self.desktop_profile_usage_refresh(
                &engine_id, &checked, refreshing, theme, window, cx,
            ));
        }
        block = block.child(title);
        for (group_index, group) in group_usage_windows(&report.windows).iter().enumerate() {
            let mut group_view = div().flex().flex_col().gap(px(6.0));
            if group_index > 0 {
                group_view = group_view.mt(px(8.0));
            }
            group_view = group_view.child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(group.cadence.title()),
            );
            for window in &group.windows {
                group_view = group_view.child(
                    self.desktop_profile_usage_meter(&engine_id, window, accent, dim, theme, cx),
                );
            }
            if let Some(duration) = reset_duration(&group.windows, now_ms) {
                // One inline run like source: the duration range carries the
                // foreground highlight so spaces and wrapping are preserved.
                let sentence = format!(
                    "Your {} limit resets in {}.",
                    group.cadence.title().to_lowercase(),
                    duration
                );
                let body = match sentence.find(duration.as_str()) {
                    Some(start) => {
                        let end = start + duration.len();
                        StyledText::new(SharedString::from(sentence)).with_highlights([(
                            start..end,
                            HighlightStyle {
                                color: Some(theme.foreground),
                                ..Default::default()
                            },
                        )])
                    }
                    None => StyledText::new(SharedString::from(sentence)),
                };
                group_view = group_view.child(
                    div()
                        .mt(px(8.0))
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(theme.secondary)
                        .child(body),
                );
            }
            block = block.child(group_view);
        }
        block
    }

    /// Inline checked/refresh control: the provider's own "last checked"
    /// reading at rest, swapping to a foreground Refresh action on hover or
    /// keyboard focus and to a spinner while its refresh is in flight. All
    /// three readings share one grid cell like the source `t-checked`
    /// grid, so the width is always the max of reading and action and the
    /// swap never shifts layout; each reading animates on the source 150ms
    /// ease-in-out opacity/blur(2px)/±4px paint offset, interrupted from the
    /// retained visual values. Withheld until the engine has answered at
    /// least once. Keyboard focus plus Enter/Space refreshes once through
    /// the same path as a click.
    fn desktop_profile_usage_refresh(
        &self,
        engine_id: &str,
        checked: &str,
        refreshing: bool,
        theme: DesktopTheme,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let group = format!("profile-usage-refresh-{engine_id}");
        let selector = format!("artisan-profile-usage-refresh-{engine_id}");
        let focus = self.profile_refresh_focus_handle(engine_id, cx);
        self.refresh_profile_swap(engine_id, refreshing, cx.reduce_motion(), window, cx);
        let swap = self
            .profile_refresh_swap
            .borrow()
            .get(engine_id)
            .cloned()
            .unwrap_or_else(RefreshSwap::resting);
        // One grid cell shared by all three readings, so the control width
        // is always the max of reading and action like the source
        // `t-checked` grid — never collapsing, never shifting on swap. Each
        // reading paints its retained opacity/blur/offset triple, so a
        // reversal keeps interpolating without sign flips. The ring only
        // attaches for keyboard-driven focus, matching source
        // `:focus-visible` (a mouse click followed by pointer leave is
        // still mouse focus and shows no ring).
        let paint = |displayed: f32, off_displayed: f32| {
            (displayed, (1.0 - displayed) * 2.0, off_displayed)
        };
        let (reading_opacity, reading_blur, reading_top) =
            paint(swap.displayed[0], swap.off_displayed[0]);
        let (action_opacity, action_blur, action_top) =
            paint(swap.displayed[1], swap.off_displayed[1]);
        let (spinner_opacity, spinner_blur, spinner_top) =
            paint(swap.displayed[2], swap.off_displayed[2]);
        let ring = vec![gpui::BoxShadow {
            color: self.theme.interaction.focus_ring_color.to_paint(),
            offset: gpui::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: self.theme.interaction.focus_ring_width,
            inset: false,
        }];
        let mut control = div()
            .id(selector.clone())
            .debug_selector({
                let selector = selector.clone();
                move || selector.clone()
            })
            .track_focus(&focus)
            .group(group)
            .grid()
            .flex_shrink_0()
            .text_size(px(12.0))
            .line_height(px(16.0));
        if focus.is_focused(window) && window.last_input_was_keyboard() {
            control = control.focus(move |style| style.shadow(ring));
        }
        control = control
            .child(
                div()
                    .col_start(1)
                    .row_start(1)
                    .relative()
                    .top(px(reading_top))
                    .whitespace_nowrap()
                    .text_color(theme.secondary)
                    .child(checked.to_owned())
                    .opacity(reading_opacity)
                    .blur(px(reading_blur)),
            )
            .child(
                div()
                    .col_start(1)
                    .row_start(1)
                    .relative()
                    .top(px(action_top))
                    .flex()
                    .items_center()
                    .justify_end()
                    .opacity(action_opacity)
                    .blur(px(action_blur))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_color(theme.foreground)
                            .child("Refresh"),
                    ),
            )
            .child(
                div()
                    .col_start(1)
                    .row_start(1)
                    .relative()
                    .top(px(spinner_top))
                    .flex()
                    .items_center()
                    .justify_end()
                    .opacity(spinner_opacity)
                    .blur(px(spinner_blur))
                    .child(
                        FadeArc::new(SharedString::from(selector.clone()), self.theme)
                            .size(px(14.0))
                            .active(refreshing)
                            .debug_selector(format!("{selector}-spinner")),
                    ),
            )
            .on_hover(cx.listener({
                let swap_engine_id = engine_id.to_owned();
                let swap_focus = focus.clone();
                move |app, hovered: &bool, window, cx| {
                    if let Some(swap) = app
                        .profile_refresh_swap
                        .borrow_mut()
                        .get_mut(swap_engine_id.as_str())
                    {
                        swap.hovered = *hovered;
                    }
                    let target = if refreshing {
                        RefreshSwapTarget::Loading
                    } else if *hovered || swap_focus.is_focused(window) {
                        RefreshSwapTarget::Action
                    } else {
                        RefreshSwapTarget::Reading
                    };
                    app.retarget_profile_swap(&swap_engine_id, target, cx.reduce_motion());
                    app.schedule_profile_swap_frame(window, cx);
                    cx.notify();
                }
            }));
        if !refreshing {
            let click_engine_id = engine_id.to_owned();
            let key_engine_id = engine_id.to_owned();
            control = control
                .cursor_pointer()
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.refresh_single_profile_engine(&click_engine_id, cx);
                }))
                .on_key_down(cx.listener(move |app, event: &gpui::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        app.refresh_single_profile_engine(&key_engine_id, cx);
                        cx.notify();
                    }
                }));
        }
        control
    }

    /// One cadence meter row: scope label plus a fixed 72x8 provider-accent
    /// meter with the source 14-tick quantization. Hovering arms the glass
    /// remaining tooltip; the exact percentage lives only there, never as
    /// inline text.
    fn desktop_profile_usage_meter(
        &self,
        engine_id: &str,
        window: &NativeUsageWindow,
        accent: gpui::Hsla,
        dim: gpui::Hsla,
        theme: DesktopTheme,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let segments = usize::from(crate::usage_meter::USAGE_METER_SEGMENTS);
        let lit_segments =
            (usage_segment_fraction(window.percent_used) * segments as f64).round() as usize;
        // Fourteen full pitches across 72px with a 2px transparent tail cut
        // from every pitch including the last, matching the source mask.
        let tip_key = (engine_id.to_owned(), window.id.clone());
        let meter_selector = format!("artisan-profile-usage-meter-{engine_id}-{}", window.id);
        let bar_selector = format!("{meter_selector}-bar");
        let mut meter = div()
            .debug_selector({
                let bar_selector = bar_selector.clone();
                move || bar_selector.clone()
            })
            .w(px(72.0))
            .h(px(8.0))
            .flex_shrink_0()
            .flex();
        for index in 0..segments {
            meter = meter.child(
                div()
                    .w(px(PROFILE_METER_TICK_PX))
                    .mr(px(2.0))
                    .h_full()
                    .bg(if index < lit_segments { accent } else { dim }),
            );
        }
        let meter_hover = Rc::clone(&self.profile_meter_hover);
        let tip_surface = Rc::clone(&self.profile_tip_surface_bounds);
        let tip_anchor = Rc::clone(&self.profile_tip_anchor);
        let probe_key = tip_key.clone();
        let meter_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                if meter_hover.borrow().as_ref() != Some(&probe_key) {
                    return;
                }
                let Some(surface) = *tip_surface.borrow() else {
                    return;
                };
                let rect = HoverRect {
                    left: f32::from(bounds.left() - surface.left()),
                    top: f32::from(bounds.top() - surface.top()),
                    width: f32::from(bounds.size.width),
                    height: f32::from(bounds.size.height),
                };
                let mut anchor = tip_anchor.borrow_mut();
                if anchor
                    .as_ref()
                    .is_some_and(|(key, current)| key == &probe_key && *current == rect)
                {
                    return;
                }
                *anchor = Some((probe_key.clone(), rect));
                window.defer(cx, |window, _| window.refresh());
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id(meter_selector.clone())
            .relative()
            .flex()
            .items_center()
            .gap(px(16.0))
            .debug_selector({
                let meter_selector = meter_selector.clone();
                move || meter_selector.clone()
            })
            .child(meter_probe)
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .pl(px(8.0))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(theme.secondary)
                    .child(window.scope_label().to_owned()),
            )
            .child(meter)
            .on_hover(cx.listener({
                let tip_key = tip_key.clone();
                let percent_used = window.percent_used;
                move |app, hovered: &bool, window, cx| {
                    if *hovered {
                        *app.profile_meter_hover.borrow_mut() = Some(tip_key.clone());
                        // One shared tween per menu-open session: the first
                        // reading runs up from just short of its value,
                        // later rows carry the displayed value across.
                        let remaining = usage_remaining_percent(percent_used) as f64;
                        {
                            let mut tween = app.profile_tip_tween.borrow_mut();
                            if !tween.seen {
                                tween.seen = true;
                                tween.displayed = tip_run_up_from(remaining);
                                tween.from = tween.displayed;
                            } else {
                                tween.from = tween.displayed;
                            }
                            tween.to = remaining;
                            tween.started_ms = profile_usage_now_ms();
                            if cx.reduce_motion() {
                                tween.displayed = remaining;
                                tween.scheduled = false;
                            }
                        }
                        app.schedule_profile_tip_frame(window, cx);
                    } else if app.profile_meter_hover.borrow().as_ref() == Some(&tip_key) {
                        app.profile_meter_hover.borrow_mut().take();
                        app.profile_tip_anchor.borrow_mut().take();
                    }
                    cx.notify();
                }
            }))
    }

    /// Glass remaining tooltip for the hovered meter, anchored beside its
    /// row with the source 8px offset and clamped into the viewport. The
    /// number is the shared tweened remaining percentage, so moving across
    /// rows carries one value onto the next.
    fn desktop_profile_usage_tooltip(
        &self,
        window: &Window,
        theme: DesktopTheme,
    ) -> Option<Stateful<Div>> {
        let (engine_id, window_id) = self.profile_meter_hover.borrow().clone()?;
        let ((anchor_engine, anchor_window), anchor) = self.profile_tip_anchor.borrow().clone()?;
        if (engine_id.clone(), window_id.clone()) != (anchor_engine, anchor_window) {
            return None;
        }
        let known = self
            .profile_usage
            .entry(&engine_id)
            .and_then(|entry| entry.report.as_ref())
            .and_then(|report| report.windows.iter().find(|window| window.id == window_id))
            .is_some();
        if !known {
            return None;
        }
        let displayed = self.profile_tip_tween.borrow().displayed;
        let viewport_width = f32::from(window.bounds().size.width);
        let surface_left = f32::from(self.profile_tip_surface_bounds.borrow().as_ref()?.left());
        let minimum_left = PROFILE_MENU_VIEWPORT_MARGIN_PX - surface_left;
        let max_left = (viewport_width - PROFILE_MENU_VIEWPORT_MARGIN_PX - 224.0 - surface_left)
            .max(minimum_left);
        let left = (anchor.left + anchor.width + 8.0).clamp(minimum_left, max_left);
        Some(
            div()
                .id("artisan-profile-usage-tooltip")
                .debug_selector(|| "artisan-profile-usage-tooltip".to_owned())
                .absolute()
                .left(px(left))
                .top(px(anchor.top))
                .max_w(px(224.0))
                .rounded(RadiusTokens::value(RadiusStep::X2l))
                .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
                .bg(glass_foreground_base(self.theme))
                .border_1()
                .border_color(theme.line)
                .shadow(glass_card_shadows())
                .child(glass_material_layer(
                    GlassStrength::Quiet,
                    RadiusTokens::value(RadiusStep::X2l),
                ))
                .child(glass_highlight_layer(
                    GlassStrength::Quiet,
                    RadiusTokens::value(RadiusStep::X2l),
                ))
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(theme.secondary)
                        .child("You have ")
                        .child(
                            div()
                                .text_color(theme.foreground)
                                .child(format!("{}%", displayed.round() as i64)),
                        )
                        .child(" left."),
                ),
        )
    }

    fn desktop_profile(&self, window: &mut Window, cx: &Context<Self>) -> Div {
        let theme = self.desktop_theme;
        let origin = self.profile_origin.clone();
        let render_avatar = || {
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
            if let Some(path) = self.profile_picture.clone() {
                gpui::img(path)
                    .size(px(32.0))
                    .rounded(px(8.0))
                    .object_fit(gpui::ObjectFit::Cover)
                    .with_fallback(fallback.clone())
                    .with_loading(fallback)
                    .into_any_element()
            } else {
                fallback()
            }
        };
        let avatar = render_avatar();
        let focus_ring = vec![gpui::BoxShadow {
            color: self.theme.interaction.focus_ring_color.to_paint(),
            offset: gpui::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: self.theme.interaction.focus_ring_width,
            inset: false,
        }];
        // The ring marks keyboard focus only: a mouse click focuses too, but
        // mouse modality never shows it, matching source `:focus-visible`.
        let keyboard_focused =
            self.profile_focus.is_focused(window) && window.last_input_was_keyboard();
        let mut trigger = div()
            .id("artisan-desktop-profile-trigger")
            .debug_selector(|| "artisan-desktop-profile-trigger".to_string())
            .track_focus(&self.profile_focus)
            .tab_index(0)
            .cursor_pointer()
            .rounded(px(8.0))
            .w_full()
            .h(px(44.0))
            .p(px(5.0));
        if keyboard_focused {
            trigger = trigger.focus(move |style| style.shadow(focus_ring));
        }
        trigger = trigger
            .flex()
            .items_center()
            .gap(px(8.0))
            .relative()
            .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                if *hovered {
                    app.sidebar_hover
                        .borrow_mut()
                        .set_active(SIDEBAR_PROFILE_HOVER_ID.to_owned());
                } else if app.sidebar_hover.borrow().active_id() == Some(SIDEBAR_PROFILE_HOVER_ID) {
                    app.sidebar_hover.borrow_mut().hide();
                }
                cx.notify();
            }))
            .child(sidebar_hover_probe(
                Rc::clone(&self.sidebar_hover),
                Rc::clone(&self.sidebar_hover_surface_bounds),
                SIDEBAR_PROFILE_HOVER_ID,
            ))
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
                            .child(
                                self.profile_name
                                    .clone()
                                    .map_or_else(|| "User".into(), |name| capitalize_label(&name)),
                            ),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(10.0))
                            .line_height(px(12.0))
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
                let was_open = app.profile_menu.is_open();
                let _ = app.profile_menu.press_trigger();
                window.focus(&app.profile_focus, cx);
                if !was_open && app.profile_menu.is_open() {
                    app.clear_profile_hover();
                    app.begin_profile_menu_open(cx);
                    app.ensure_profile_usage(false, None, cx);
                } else {
                    app.clear_profile_hover();
                    app.cancel_profile_usage_scroll();
                    if was_open {
                        app.begin_profile_menu_close(cx);
                    }
                }
                cx.notify();
            }))
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        let was_open = app.profile_menu.is_open();
                        let _ = app.profile_menu.dismiss();
                        app.clear_profile_hover();
                        app.cancel_profile_usage_scroll();
                        if was_open {
                            app.begin_profile_menu_close(cx);
                        }
                    }
                    "down" => {
                        let was_open = app.profile_menu.is_open();
                        app.profile_menu.set_open(true);
                        let _ = app.profile_menu.move_next();
                        app.sync_profile_hover_to_highlight();
                        if !was_open {
                            app.begin_profile_menu_open(cx);
                        }
                        app.ensure_profile_usage(false, None, cx);
                    }
                    "up" => {
                        let was_open = app.profile_menu.is_open();
                        app.profile_menu.set_open(true);
                        let _ = app.profile_menu.move_previous();
                        app.sync_profile_hover_to_highlight();
                        if !was_open {
                            app.begin_profile_menu_open(cx);
                        }
                        app.ensure_profile_usage(false, None, cx);
                    }
                    "home" => {
                        let _ = app.profile_menu.move_first();
                        app.sync_profile_hover_to_highlight();
                    }
                    "end" => {
                        let _ = app.profile_menu.move_last();
                        app.sync_profile_hover_to_highlight();
                    }
                    "enter" | "space" => {
                        if app.profile_menu.is_open() {
                            let _ = app.profile_menu.activate_highlighted();
                            app.activate_profile_selection(window, cx);
                        } else {
                            let _ = app.profile_menu.press_trigger();
                            if app.profile_menu.is_open() {
                                app.clear_profile_hover();
                                app.begin_profile_menu_open(cx);
                                app.ensure_profile_usage(false, None, cx);
                            }
                        }
                    }
                    "tab" => {
                        let was_open = app.profile_menu.is_open();
                        let _ = app.profile_menu.dismiss();
                        app.clear_profile_hover();
                        app.cancel_profile_usage_scroll();
                        if was_open {
                            app.begin_profile_menu_close(cx);
                        }
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
        // The retained exit presentation stays mounted through Closing so
        // the shared 100ms fade/slide-out can complete, exactly like the
        // model picker popover.
        let menu_phase = self.profile_menu_motion.borrow().phase();
        if self.profile_menu.is_open() || menu_phase == PickerMenuPhase::Closing {
            // The dropdown paints from the shared Artisan text tokens rather
            // than the desktop shell's custom palette; the bottom trigger
            // keeps the shell palette.
            let theme = DesktopTheme {
                foreground: self.theme.colors.foreground.to_paint(),
                secondary: self.theme.colors.muted_foreground.to_paint(),
                ..theme
            };
            // Source `bg-border/50` resolved from the shared border token.
            let separator = self.profile_separator_paint();
            // The machine line only paints when the hostname differs from
            // the profile name, matching `show_hostname` in source.
            let show_profile_hostname = self.profile_hostname.as_deref().is_some_and(|hostname| {
                self.profile_name
                    .as_deref()
                    .is_none_or(|name| name != hostname)
            });
            let tip_surface = Rc::clone(&self.profile_tip_surface_bounds);
            let tip_surface_probe = canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| {
                    let changed = {
                        let mut surface = tip_surface.borrow_mut();
                        if *surface == Some(bounds) {
                            false
                        } else {
                            *surface = Some(bounds);
                            true
                        }
                    };
                    if changed {
                        window.defer(cx, |window, _| window.refresh());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let mut panel = div()
                .id("artisan-desktop-profile-menu")
                .debug_selector(|| "artisan-desktop-profile-menu".to_string())
                .min_w(px(256.0))
                .max_w(px(352.0))
                .rounded(RadiusTokens::value(RadiusStep::X2l))
                .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
                .bg(glass_foreground_base(self.theme))
                .border_1()
                .border_color(theme.line)
                .shadow(glass_card_shadows())
                .flex()
                .flex_col()
                .relative()
                .child(glass_material_layer(
                    GlassStrength::Quiet,
                    RadiusTokens::value(RadiusStep::X2l),
                ))
                .child(glass_highlight_layer(
                    GlassStrength::Quiet,
                    RadiusTokens::value(RadiusStep::X2l),
                ))
                .child(tip_surface_probe)
                .block_mouse_except_scroll()
                .on_mouse_down_out(cx.listener(|app, event: &gpui::MouseDownEvent, _, cx| {
                    let trigger = app.profile_origin.get();
                    if !trigger.contains(&event.position) {
                        let was_open = app.profile_menu.is_open();
                        let _ = app.profile_menu.dismiss();
                        app.clear_profile_hover();
                        app.cancel_profile_usage_scroll();
                        if was_open {
                            app.begin_profile_menu_close(cx);
                        }
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .debug_selector(|| "artisan-desktop-profile-header".to_owned())
                        .px(px(12.0))
                        .py(px(16.0))
                        .flex()
                        .items_center()
                        .gap(px(12.0))
                        .child(
                            div()
                                .size(px(32.0))
                                .flex_shrink_0()
                                .rounded(px(8.0))
                                .overflow_hidden()
                                .child(render_avatar()),
                        )
                        .child(
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
                                        .line_height(px(20.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.foreground)
                                        .child(self.profile_name.clone().map_or_else(
                                            || "Not connected".into(),
                                            |name| capitalize_label(&name),
                                        )),
                                )
                                .children(show_profile_hostname.then(|| {
                                    div()
                                        .truncate()
                                        .text_size(px(12.0))
                                        .line_height(px(16.0))
                                        .text_color(theme.secondary)
                                        .child(
                                            self.profile_hostname
                                                .clone()
                                                .unwrap_or_else(|| "This computer".to_owned()),
                                        )
                                })),
                        ),
                )
                .child(div().h(px(1.0)).bg(separator).my(px(4.0)))
                .child(
                    div()
                        .id("artisan-profile-usage-scroll")
                        .debug_selector(|| "artisan-profile-usage-scroll".to_owned())
                        .min_h(px(0.0))
                        .max_h(self.profile_usage_max_height(window))
                        .overflow_y_scroll()
                        .track_scroll(&self.profile_usage_scroll)
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .min_w(px(0.0))
                                .flex_shrink_0()
                                .on_scroll_wheel(
                                    cx.listener(Self::handle_profile_usage_scroll_wheel),
                                )
                                .child(self.desktop_profile_usage(theme, window, cx)),
                        ),
                )
                .child(div().h(px(1.0)).bg(separator).my(px(4.0)));
            self.profile_hover.borrow_mut().clear_if_missing(&[
                PROFILE_SETTINGS_HOVER_ID.to_owned(),
                PROFILE_USAGE_HOVER_ID.to_owned(),
            ]);
            let profile_hover = Rc::clone(&self.profile_hover);
            let profile_hover_surface = Rc::clone(&self.profile_hover_surface_bounds);
            let profile_surface_probe = {
                let surface = Rc::clone(&profile_hover_surface);
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let changed = {
                            let mut surface = surface.borrow_mut();
                            if *surface == Some(bounds) {
                                false
                            } else {
                                *surface = Some(bounds);
                                true
                            }
                        };
                        if changed {
                            window.defer(cx, |window, _| window.refresh());
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            };
            let profile_row_hover = Rc::clone(&profile_hover);
            let profile_row_surface = Rc::clone(&profile_hover_surface);
            let profile_hover_probe = move |id: &'static str| {
                let measured_id = id.to_owned();
                let hover = Rc::clone(&profile_row_hover);
                let surface_bounds = Rc::clone(&profile_row_surface);
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
            };
            let mut actions = div()
                .id("artisan-desktop-profile-actions-hover-surface")
                .debug_selector(|| "artisan-desktop-profile-actions-hover-surface".to_owned())
                .relative()
                .w_full()
                .flex()
                .flex_col()
                .p(px(4.0))
                .on_hover(cx.listener(|app, hovered: &bool, _, cx| {
                    if !*hovered && !app.profile_hover_keyboard.get() {
                        app.profile_hover.borrow_mut().clear();
                        cx.notify();
                    }
                }))
                .child(profile_surface_probe)
                .child(render_picker_hover_pill(
                    self.theme,
                    Rc::clone(&profile_hover),
                    "profile",
                    RadiusTokens::value(RadiusStep::Xl),
                    cx.reduce_motion(),
                ));
            for (index, (label, icon, hover_id)) in [
                (
                    "Settings",
                    AssetId::TABLER_SETTINGS,
                    PROFILE_SETTINGS_HOVER_ID,
                ),
                (
                    "Usage",
                    AssetId::TABLER_LIST_DETAILS,
                    PROFILE_USAGE_HOVER_ID,
                ),
            ]
            .into_iter()
            .enumerate()
            {
                let row_selector = format!("artisan-desktop-profile-action-{index}");
                let row_probe = profile_hover_probe(hover_id);
                let row_hover_id = hover_id.to_owned();
                actions = actions.child(
                    div()
                        .id(("artisan-profile-action", index))
                        .debug_selector(move || row_selector.clone())
                        .relative()
                        .px(px(12.0))
                        .py(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(RadiusTokens::value(RadiusStep::Xl))
                        .cursor_pointer()
                        .child(row_probe)
                        .child(desktop_nav_glyph(icon, theme))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .line_height(px(20.0))
                                .text_color(theme.foreground)
                                .child(label),
                        )
                        .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                            if *hovered {
                                app.set_profile_highlight(index);
                                app.profile_hover
                                    .borrow_mut()
                                    .set_active(row_hover_id.clone());
                                app.profile_hover_keyboard.set(false);
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            if !app.profile_menu_is_interactive() {
                                return;
                            }
                            cx.stop_propagation();
                            let _ = app.profile_menu.activate_index(index);
                            app.activate_profile_selection(window, cx);
                        })),
                );
            }
            panel = panel.child(actions);
            if let Some(tooltip) = self.desktop_profile_usage_tooltip(window, theme) {
                panel = panel.child(tooltip);
            }
            let motion = *self.profile_menu_motion.borrow();
            root = root.child(gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Anchor::BottomLeft)
                    .position(self.profile_origin.get().origin)
                    .offset(gpui::point(px(0.0), px(-4.0)))
                    .child(animate_picker_menu(
                        panel,
                        Rc::clone(&self.profile_menu_motion),
                        motion,
                        "profile",
                    )),
            ));
        }
        root
    }

    fn desktop_route_title(&self) -> String {
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

    fn desktop_route_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let route = self.route().clone();
        let route_selector = route.selector_suffix();
        let content = self.route_surface(window, cx);
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
        crate::native_composer_queue::project_controls_snapshot(
            &self.composer_queue.state,
            &mut snapshot,
        );
        let queue = &self.composer_queue.state;
        let count = crate::native_composer_queue::queue_count_label(queue);
        let status = queue.status().label();
        snapshot.queue_status = if status.is_empty() {
            count
        } else {
            Some(match count {
                Some(count) => format!("{count} · {status}"),
                None => status.to_owned(),
            })
        };
        snapshot.queue_retry = if queue.can_retry_restore() {
            Some("Restore".into())
        } else if queue.can_retry_recalled_read()
            || matches!(
                queue.status(),
                crate::composer_queue_state::QueueStatus::TransportFailed
            )
        {
            Some("Retry".into())
        } else {
            None
        };
        // Surface the dispatcher's actual diagnostic for the oldest waiting
        // row next to the count. This is the persisted `last_error` the
        // dispatcher stored on requeue — never an inference from absent
        // frontend data.
        if !snapshot.run_active
            && matches!(queue.status(), crate::composer_queue_state::QueueStatus::Idle)
            && (queue.total_count() > 0 || !queue.entries().is_empty())
        {
            let error = queue.entries().iter().find_map(|entry| entry.dispatch_error());
            if let Some(error) = error {
                snapshot.queue_status = Some(
                    match crate::native_composer_queue::queue_count_label(queue) {
                        Some(count) => format!("{count} · {error}"),
                        None => error.to_owned(),
                    },
                );
            }
        }

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
        if let Some(message) = self.composer_model_run_error.clone() {
            snapshot.failure = Some(crate::native_composer_controls::NativeComposerFailure::new(
                0,
                "Could not start with this model",
                message,
                false,
            ));
        }
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
            .and_then(|config| match config.selection() {
                artisan_domain::EngineSelection::OpenCode2(selection) => {
                    Some(selection.model_id().as_str().to_owned())
                }
                other => other.model_id().map(|model| model.as_str().to_owned()),
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

    /// Resolves the displayed model policy to its durable engine
    /// configuration for a first send: either the explicit choice for this
    /// thread or the selector's current policy.
    ///
    /// Native choices without an explicit profile persist under the supported
    /// default profile, and admission runs against the readiness-overlaid
    /// catalog so a probed ambient account needs no managed registry.
    /// Returns the static blocking message when no policy is displayed or
    /// the displayed policy cannot become a run configuration.
    fn first_send_config(&self, cx: &App) -> Result<artisan_domain::EngineRunConfig, String> {
        let displayed = match &self.composer_model_choice {
            Some((thread, policy)) if thread == &self.selected_thread => Some(policy.clone()),
            _ => self.model_selector.read(cx).state().policy().cloned(),
        };
        let Some(raw_policy) = displayed else {
            return Err("Select a model before sending. Your draft is preserved.".to_owned());
        };
        // The harness must be runnable before anything is persisted: an
        // unrunnable engine would only requeue after the save lands. The
        // reason names the probed account state instead of a catch-all.
        let policy = crate::composer_model_config::with_default_native_profile(&raw_policy);
        let catalog = self.effective_catalog_snapshot(cx);
        catalog
            .admit_policy(&policy)
            .map_err(|_| self.readiness_block_reason(&policy.engine_id))?;
        crate::composer_model_config::config_for_policy(
            &catalog,
            &policy,
            self.engine_settings.authoritative_config(),
        )
        .map_err(|reason| reason.to_owned())
    }

    /// Admits a first send on a thread without a persisted engine
    /// configuration by persisting the displayed model policy and holding
    /// the send for its authoritative acknowledgment (continued by
    /// [`Self::continue_pending_first_send`]).
    fn admit_first_send(&mut self, cx: &mut Context<Self>) -> FirstSendAdmission {
        if self.engine_settings.authoritative_config().is_some() {
            self.suppress_stale_pending_first_send(cx);
            self.composer_model_run_error = None;
            return FirstSendAdmission::Proceed;
        }
        self.suppress_stale_pending_first_send(cx);
        if self.pending_first_send.is_some() {
            // A held send without a live save means its save died without
            // an acknowledgment: release the held flight and re-evaluate so
            // this send re-saves instead of stranding on a stale "saving"
            // message.
            if self.engine_settings.pending_save_request_id().is_none() {
                if let Some(pending) = self.pending_first_send.take() {
                    self.finish_composer_submission(pending.token, DraftDisposition::Retained, cx);
                }
            } else {
                self.composer_model_run_error = Some(
                    "Saving this model's settings, then sending. Your draft is preserved."
                        .to_owned(),
                );
                self.sync_composer_controls(cx);
                cx.notify();
                return FirstSendAdmission::Held;
            }
        }
        let expected = match self.first_send_config(cx) {
            Ok(config) => config,
            Err(message) => {
                self.composer_model_run_error = Some(message);
                self.sync_composer_controls(cx);
                cx.notify();
                return FirstSendAdmission::Held;
            }
        };
        // Adopt a matching in-flight save (for example the auto-save the
        // policy selection just issued) instead of issuing a duplicate.
        if let Some((pending_thread, retained)) = self
            .engine_settings
            .pending_save()
            .map(|(thread, config)| (thread.clone(), config.clone()))
            && self.selected_thread.as_ref() == Some(&pending_thread)
            && retained == expected
        {
            self.begin_pending_first_send(cx);
            return FirstSendAdmission::Held;
        }
        if self.engine_settings.pending_save_request_id().is_some() {
            self.composer_model_run_error = Some(
                "This model's settings have not been saved yet. Your draft is preserved; try again once saving finishes."
                    .to_owned(),
            );
            self.sync_composer_controls(cx);
            cx.notify();
            return FirstSendAdmission::Held;
        }
        // Admission rests on the backend-probed account verdict: request a
        // fresh read for the displayed engine before evaluating it, so the
        // first send at startup, on selection, and from Settings observes
        // true readiness instead of an empty row.
        let displayed_engine = match &self.composer_model_choice {
            Some((thread, policy)) if thread == &self.selected_thread => {
                Some(policy.engine_id.clone())
            }
            _ => self
                .model_selector
                .read(cx)
                .state()
                .policy()
                .map(|policy| policy.engine_id.clone()),
        };
        if let Some(engine_id) = displayed_engine.as_deref() {
            self.ensure_profile_usage(false, Some(engine_id), cx);
        }
        // The settings draft stays `OpenCode` 2-shaped until per-engine
        // settings UI lands, so native selections cannot travel through it:
        // save the validated configuration directly with an `Unconfigured`
        // precondition instead.
        let Some(thread_id) = self.selected_thread.clone() else {
            return FirstSendAdmission::Held;
        };
        if !self.submit_first_send_save(thread_id, expected) {
            self.composer_model_run_error = Some(
                "Engine settings could not be saved. Your draft is preserved; retry the model selection."
                    .to_owned(),
            );
            self.sync_composer_controls(cx);
            cx.notify();
            return FirstSendAdmission::Held;
        }
        self.sync_composer_model_policy(cx);
        self.begin_pending_first_send(cx);
        FirstSendAdmission::Held
    }

    fn begin_message_submission(&mut self, cx: &mut Context<Self>) {
        if !self.message_submission_is_admissible(cx) || self.message_flight.is_some() {
            return;
        }
        // The choice-versus-saved check is only meaningful once a thread
        // carries a persisted configuration. On an unconfigured thread it
        // would always fail and strand explicit selections before the
        // first-send save flow below; that flow owns unconfigured sends.
        if self.engine_settings.authoritative_config().is_some()
            && let Some((thread, policy)) = &self.composer_model_choice
            && thread == &self.selected_thread
        {
            self.composer_model_run_error = crate::composer_model_config::validate_run_choice(
                &self.effective_catalog_snapshot(cx),
                policy,
                self.engine_settings.authoritative_config(),
            )
            .err()
            .map(|reason| reason.to_owned());
            if self.composer_model_run_error.is_some() {
                self.sync_composer_controls(cx);
                cx.notify();
                return;
            }
        }
        // First-send admission is owned by `admit_first_send`: on an
        // unconfigured thread it persists the displayed model policy and
        // holds this send for the authoritative save acknowledgment.
        if !matches!(self.admit_first_send(cx), FirstSendAdmission::Proceed) {
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

    /// Begins the composer flight for a first send whose save is admitted
    /// and holds it as the pending first send. The transport command is
    /// issued only by [`Self::continue_pending_first_send`] once the
    /// authoritative save acknowledgment arrives.
    fn begin_pending_first_send(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        self.clear_message_retry();
        let (body, token) = match self
            .composer
            .update(cx, |composer, _| composer.begin_payload_submission())
        {
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
        self.pending_first_send = Some(PendingFirstSend {
            thread_id,
            payload: body,
            token,
        });
        self.composer_model_run_error = Some(
            "Saving this model's settings, then sending. Your draft is preserved.".to_owned(),
        );
        self.sync_composer_availability(cx);
        cx.notify();
    }

    /// Suppresses a pending first send whose thread no longer owns the
    /// composer, retaining its draft. Returns whether one was suppressed.
    fn suppress_stale_pending_first_send(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pending) = self.pending_first_send.as_ref() else {
            return false;
        };
        if self.selected_thread.as_ref() == Some(&pending.thread_id) {
            return false;
        }
        let pending = self.pending_first_send.take().expect("pending checked above");
        self.finish_composer_submission(pending.token, DraftDisposition::Retained, cx);
        self.composer_model_run_error = None;
        self.sync_composer_availability(cx);
        true
    }

    /// Continues a pending first send after its save was acknowledged.
    ///
    /// The queued command carries the exact payload captured at send time
    /// only while the same thread is still selected and the draft still
    /// matches it; any thread switch or draft change suppresses the send and
    /// retains the current draft instead of queueing stale text.
    fn continue_pending_first_send(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_first_send.take() else {
            return;
        };
        let sendable = self.selected_thread.as_ref() == Some(&pending.thread_id)
            && self.message_flight.is_none()
            && self
                .composer
                .read(cx)
                .draft_matches_payload(&pending.payload);
        if !sendable {
            self.finish_composer_submission(pending.token, DraftDisposition::Retained, cx);
            self.composer_model_run_error = None;
            self.sync_composer_availability(cx);
            cx.notify();
            return;
        }
        self.message_receipt = None;
        self.message_failure = None;
        self.composer_model_run_error = None;
        let request_id = match create_message_request_id() {
            Ok(request_id) => request_id,
            Err(failure) => {
                self.reject_message_submission(pending.token, failure, cx);
                return;
            }
        };
        let command =
            NativeTransportCommand::QueueMessage(Box::new(artisan_domain::QueueMessage {
                request_id: request_id.clone(),
                thread_id: pending.thread_id.clone(),
                payload: pending.payload.clone(),
            }));
        match self.submit_command(command) {
            Ok(()) => {
                self.message_flight = Some(NativeMessageFlight {
                    thread_id: pending.thread_id,
                    request_id,
                    payload: pending.payload,
                    token: pending.token,
                });
            }
            Err(error) => {
                self.reject_message_submission(pending.token, command_failure(error), cx);
                return;
            }
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }

    /// Suppresses a pending first send after its save failed, retaining its
    /// draft and naming the failed save.
    fn fail_pending_first_send(&mut self, cx: &mut Context<Self>) {
        let Some(pending) = self.pending_first_send.take() else {
            return;
        };
        self.finish_composer_submission(pending.token, DraftDisposition::Retained, cx);
        self.composer_model_run_error = Some(
            "Engine settings could not be saved. Your draft is preserved; retry the model selection."
                .to_owned(),
        );
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
        if let Some(pending) = self.pending_first_send.take() {
            self.finish_composer_submission(pending.token, DraftDisposition::Retained, cx);
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

    /// Drains one queued answer batch into live transport, once per tick.
    ///
    /// Runs at the head of the controller tick beside the sibling drains, so
    /// a slow or failing transport cannot stall unrelated per-tick work.
    /// Admission follows the established submit path: without it the outbox
    /// is left untouched. Each taken dispatch submits once with its
    /// already-minted request id; `Busy`/`Stopped` keep rows pending with the
    /// existing retry/diagnostic texts, and single-flight holds until
    /// receipts pair through the existing settle-in-place pairing. Draining
    /// first also keeps a same-tick host retirement from dropping gestures.
    fn drain_answer_dispatches(&mut self, cx: &mut Context<Self>) {
        if !self.command_submission_is_available() {
            return;
        }
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        let surface = host.read(cx).surface().clone();
        let this = &*self;
        surface.update(cx, |surface, _| {
            surface.drain_pending_answer_dispatches(&mut |command| this.submit_command(command));
        });
    }

    fn poll_service(&mut self, cx: &mut Context<Self>) -> bool {
        self.drain_answer_dispatches(cx);
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
            NativeTransportEvent::ComposerState(event) => {
                self.handle_composer_state_event(event, cx)
            }
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
                self.reset_profile_usage_for_connection();
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
                self.reset_profile_usage_for_connection();
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
            NativeTransportEvent::AccountUsage {
                engine_id,
                generation,
                request_seq,
                entry,
            } => self.handle_account_usage(engine_id, generation, request_seq, entry, cx),
            NativeTransportEvent::AccountUsageFailed {
                engine_id,
                generation,
                request_seq,
                failure,
            } => self.handle_account_usage_failed(engine_id, generation, request_seq, failure, cx),
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
            // Answer receipts pair through the engine approve pairing in a
            // later packet; the transport delivers them here but no gate
            // consumes them yet.
            NativeTransportEvent::ApprovalAnswered(_)
            | NativeTransportEvent::ApprovalFailed { .. }
            | NativeTransportEvent::QuestionAnswered(_)
            | NativeTransportEvent::QuestionFailed { .. } => {}
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
            NativeTransportEvent::EngineObservation(observation) => {
                self.handle_engine_observation(&observation, cx);
            }
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
        self.reset_profile_usage_for_connection();
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

    /// Pairs one uni-stream engine observation into presentation state.
    ///
    /// Only the selected thread's rows are retained; events for any other
    /// thread are ignored. Cursor ordering and reconnect dedup are owned by
    /// [`EngineObservationState`], which is independent of host mounting, so
    /// unlike patch batches this path does not wait for a thread-switch
    /// flight to settle. This path never issues commands: approvals and
    /// questions render with their request ids for the later answer packet,
    /// but no answer is dispatched here.
    fn handle_engine_observation(&mut self, observation: &ServerEvent, cx: &mut Context<Self>) {
        let artisan_domain::Event::EngineObservation(paired) = &observation.event else {
            return;
        };
        if self.selected_thread.as_ref() != Some(&paired.thread_id) {
            return;
        }
        let same_thread = self
            .engine_observations
            .as_ref()
            .is_some_and(|retained| retained.thread_id() == &paired.thread_id);
        if !same_thread {
            self.engine_observations = Some(EngineObservationState::new(paired.thread_id.clone()));
        }
        if let Some(state) = self.engine_observations.as_mut() {
            let outcome = state.apply(observation.cursor.get(), paired);
            if matches!(outcome, ApplyOutcome::Applied { .. }) {
                cx.notify();
            }
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
        self.install_picker(options.clone(), self.selected_project.clone(), cx);
        self.install_home_picker(options, self.selected_project.clone(), cx);
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
        self.install_picker(options.clone(), self.selected_project.clone(), cx);
        self.install_home_picker(options, self.selected_project.clone(), cx);
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
        self.install_picker(options.clone(), Some(project_id.clone()), cx);
        self.install_home_picker(options, Some(project_id), cx);
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
        self.install_picker(options.clone(), self.selected_project.clone(), cx);
        self.install_home_picker(options, self.selected_project.clone(), cx);
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
        if let Some(picker) = self.picker.clone() {
            picker.update(cx, |picker, picker_cx| {
                picker.set_disabled(disabled, picker_cx);
            });
        }
        if let Some(home_picker) = self.home_picker.clone() {
            home_picker.update(cx, |picker, picker_cx| {
                picker.set_disabled(disabled, picker_cx);
            });
        }
    }

    /// Installs the home-surface inline switcher over the same catalog and
    /// current project as the sidebar picker, observed through the shared
    /// picker-action routing.
    fn install_home_picker(
        &mut self,
        options: Vec<ProjectOption>,
        current: Option<ProjectId>,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.new(|picker_cx| {
            HomeProjectPickerView::new(options, current, self.desktop_theme, picker_cx)
        });
        let subscription = cx.observe(&picker, |application, picker, cx| {
            application.route_home_picker_action(&picker, cx);
        });
        self.home_picker = Some(picker);
        drop(self.home_picker_subscription.replace(subscription));
        self.sync_thread_picker_disabled(cx);
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
        self.route_picker_action_inner(action, cx);
    }

    fn route_home_picker_action(
        &mut self,
        picker: &Entity<HomeProjectPickerView>,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = picker.read(cx).last_action() else {
            return;
        };
        self.route_picker_action_inner(action, cx);
    }

    /// Shared admission-deduped routing for both project picker surfaces.
    fn route_picker_action_inner(&mut self, action: ProjectPickerAction, cx: &mut Context<Self>) {
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

    /// Returns the selector snapshot overlaid with backend-probed account
    /// readiness.
    ///
    /// Static native models whose engine carries a fresh authenticated usage
    /// report are admittable without any managed `OpenCode` profile or
    /// registry; the overlay recomputes that gated subset on every call so a
    /// signed-out, failed, or stale engine never inherits a previous
    /// admission from the stored snapshot.
    fn effective_catalog_snapshot(&self, cx: &App) -> NativeModelCatalog {
        let snapshot = self.model_selector.read(cx).state().snapshot().clone();
        catalog_with_usage_readiness(snapshot, &self.profile_usage, profile_usage_now_ms())
    }

    /// Returns the actionable reason a displayed policy's engine cannot run.
    ///
    /// The verdict comes from the backend-probed usage row, never from a
    /// catch-all, and the wording is shared across engines: a signed-out
    /// engine names sign-in, a pending read says a check is running, and any
    /// other unavailable state reports the check status plus the actual
    /// probed failure when one exists. Nothing here claims a missing
    /// installation or broken binary without executable evidence — a stale,
    /// failed, or never-probed check reads as a status problem with a
    /// refresh recovery, since the installed authenticated account is the
    /// established baseline. Every message preserves the draft and points
    /// at the working recovery (the model retry that refreshes account
    /// status, or Settings → Engines). A `Ready` engine that the catalog
    /// still rejects is a catalog-side unavailability, not an account
    /// problem.
    fn readiness_block_reason(&self, engine_id: &str) -> String {
        let label = profile_usage_display_name(engine_id);
        match engine_readiness(&self.profile_usage, engine_id, profile_usage_now_ms()) {
            EngineReadiness::Ready => "This model is unavailable in the runtime catalog right now. Your draft is preserved; retry or pick another model.".to_owned(),
            EngineReadiness::NeedsSignIn => format!(
                "{label} account sign-in is required. Your draft is preserved; open Settings → Engines → {label} to review it, or retry to refresh."
            ),
            EngineReadiness::Checking => format!(
                "Checking the {label} account status. Your draft is preserved; retry in a moment."
            ),
            EngineReadiness::NotReady => {
                match engine_refresh_failure(&self.profile_usage, engine_id) {
                    Some(failure) => format!(
                        "{label} account status is unavailable: {failure}. Your draft is preserved; retry to refresh, or open Settings → Engines → {label}."
                    ),
                    None => format!(
                        "{label} account status is unavailable right now. Your draft is preserved; retry to refresh its status, or open Settings → Engines → {label}."
                    ),
                }
            }
        }
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
            .map(|config| config.selection().profile_id().clone())
            .or_else(|| match self.engine_settings.registry_view() {
                crate::engine_settings::RegistryView::Present(profiles) if profiles.len() == 1 => {
                    profiles.into_iter().next()
                }
                _ => None,
            });
        let Some(profile_id) = profile else {
            self.reset_composer_catalog(cx);
            // Unconfigured threads without a registry profile still need the
            // probed account verdict: static native models admit from usage
            // readiness alone, without any backend catalog read.
            self.ensure_profile_usage(false, None, cx);
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
        self.refresh_settings_engine_snapshot(cx);
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
            self.refresh_settings_engine_snapshot(cx);
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

    /// Returns whether the Forge connection can admit an account-usage read.
    fn profile_usage_connected(&self) -> bool {
        #[cfg(test)]
        if self.test_command_sink.is_some() {
            return !self.service_stopped && !self.shutdown_prepared;
        }
        self.service
            .as_ref()
            .is_some_and(|service| !service.is_finished())
            && !self.service_stopped
            && !self.shutdown_prepared
    }

    /// Advances the connection scope and drops incompatible cache/pending.
    fn reset_profile_usage_for_connection(&mut self) {
        let next = self
            .profile_usage_generation
            .checked_next()
            .unwrap_or(ProfileUsageGeneration::first());
        self.profile_usage_generation = next;
        self.profile_usage.clear_for_connection();
    }

    /// Ensures per-engine usage for an opened menu.
    ///
    /// Missing and stale (180s) rows are dispatched independently with named
    /// pending rows; fresh rows are retained. `force` bypasses freshness and
    /// keeps the menu open. Each dispatch carries the current connection
    /// generation plus a per-engine request sequence so an older same-engine
    /// reply arriving after a forced refresh cannot settle or replace the
    /// newer request.
    fn ensure_profile_usage(
        &mut self,
        force: bool,
        only_engine_id: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if !self.profile_usage_connected() {
            return;
        }
        let now_ms = profile_usage_now_ms();
        let wanted = plan_profile_usage_loads(&self.profile_usage, now_ms, force, only_engine_id);
        if wanted.is_empty() {
            return;
        }
        let generation = self.profile_usage_generation;
        for engine_id in wanted {
            let Some(request_seq) = self.profile_usage_next_seq.checked_add(1) else {
                let display_name = profile_usage_display_name(&engine_id).to_owned();
                self.profile_usage.accept_failure(
                    &engine_id,
                    &display_name,
                    invalid_service_failure().to_string(),
                    None,
                );
                continue;
            };
            self.profile_usage_next_seq = request_seq;
            let display_name = profile_usage_display_name(&engine_id).to_owned();
            if self.profile_usage.entry(&engine_id).is_none() {
                self.profile_usage.entries.push(NativeUsageEntry::pending(
                    engine_id.clone(),
                    display_name.clone(),
                ));
            }
            self.profile_usage
                .begin_refresh_seq(&engine_id, request_seq);
            let command = NativeTransportCommand::ReadAccountUsage {
                engine_id: engine_id.clone(),
                generation,
                request_seq,
                force,
            };
            if let Err(error) = self.submit_command(command) {
                let failure = command_failure(error);
                self.profile_usage
                    .finish_refresh_seq(&engine_id, request_seq);
                self.profile_usage.accept_failure(
                    &engine_id,
                    &display_name,
                    failure.to_string(),
                    None,
                );
            }
        }
        cx.notify();
    }

    /// Refreshes one provider row from its explicit refresh control.
    fn refresh_single_profile_engine(&mut self, engine_id: &str, cx: &mut Context<Self>) {
        self.ensure_profile_usage(true, Some(engine_id), cx);
    }

    fn handle_account_usage(
        &mut self,
        engine_id: String,
        generation: ProfileUsageGeneration,
        request_seq: u64,
        entry: NativeUsageEntry,
        cx: &mut Context<Self>,
    ) {
        if !account_usage_response_current(
            &self.profile_usage,
            generation,
            self.profile_usage_generation,
            &engine_id,
            request_seq,
        ) || entry.engine_id != engine_id
        {
            return;
        }
        self.profile_usage.try_accept(entry, request_seq);
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    fn handle_account_usage_failed(
        &mut self,
        engine_id: String,
        generation: ProfileUsageGeneration,
        request_seq: u64,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        if !account_usage_response_current(
            &self.profile_usage,
            generation,
            self.profile_usage_generation,
            &engine_id,
            request_seq,
        ) {
            return;
        }
        let display_name = self
            .profile_usage
            .entry(&engine_id)
            .map(|entry| {
                entry
                    .report
                    .as_ref()
                    .map_or(entry.display_name.clone(), |report| {
                        report.display_name.clone()
                    })
            })
            .unwrap_or_else(|| profile_usage_display_name(&engine_id).to_owned());
        self.profile_usage.try_accept_failure(
            &engine_id,
            &display_name,
            failure.to_string(),
            None,
            request_seq,
        );
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    fn request_engine_settings_for_selected(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        // Thread selection owns the readiness refresh alongside the settings
        // and registry reads: the composer gate below evaluates the probed
        // verdict, so selection must request it rather than inheriting
        // whatever the profile popover last loaded.
        self.ensure_profile_usage(false, None, cx);
        if self.engine_settings.needs_registry_load() {
            self.submit_registry_load();
        }
        if self.engine_settings.needs_settings_load()
            || self.engine_settings.pending_reload_thread().is_some()
        {
            self.submit_settings_load(thread_id);
        }
        self.refresh_settings_engine_snapshot(cx);
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
        self.refresh_settings_engine_snapshot(cx);
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
            self.continue_pending_first_send(cx);
        }
        self.refresh_settings_engine_snapshot(cx);
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
        if accepted {
            self.fail_pending_first_send(cx);
        }
        if accepted && self.engine_settings.pending_reload_thread().is_some() {
            self.reset_composer_catalog(cx);
        }
        if self.engine_settings.pending_reload_thread().is_some() {
            self.request_engine_settings_for_selected(cx);
        } else {
            self.refresh_settings_engine_snapshot(cx);
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
        let accepted = self.engine_settings.selected_thread() == Some(thread_id)
            && self.engine_settings.pending_save_request_id() == Some(request_id);
        self.engine_settings
            .on_save_failed(thread_id, request_id, failure);
        if accepted {
            self.fail_pending_first_send(cx);
        }
        self.sync_composer_model_policy(cx);
        self.refresh_settings_engine_snapshot(cx);
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
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    /// Returns the displayed model policy for one engine, if the composer
    /// choice or the selector policy names it.
    ///
    /// The explicit choice wins over the selector default; the returned
    /// policy carries the default native profile so Settings saves observe
    /// the same durable identity as composer saves.
    fn displayed_policy_for_engine(
        &self,
        engine_id: &str,
        cx: &App,
    ) -> Option<crate::native_model_selector::SelectPolicy> {
        if let Some((thread, choice)) = self.composer_model_choice.as_ref()
            && thread == &self.selected_thread
            && choice.engine_id == engine_id
        {
            return Some(choice.clone());
        }
        let policy = self.model_selector.read(cx).state().policy().cloned()?;
        (policy.engine_id == engine_id).then_some(
            crate::composer_model_config::with_default_native_profile(&policy),
        )
    }

    /// Builds the live engine snapshot for one engine settings page.
    ///
    /// Every row is projected from current application state — the
    /// readiness-overlaid catalog, the probed usage rows, the managed
    /// registry view, and the thread configuration — so the page paints
    /// loaded, loading, unavailable, and sign-in states without inventing
    /// installation facts.
    fn settings_engine_snapshot(
        &self,
        engine_id: &str,
        cx: &App,
    ) -> SettingsEngineSnapshot {
        let now_ms = profile_usage_now_ms();
        let readiness = engine_readiness(&self.profile_usage, engine_id, now_ms);
        let entry = self.profile_usage.entry(engine_id);
        let account_email = entry
            .as_ref()
            .and_then(|entry| {
                entry
                    .report
                    .as_ref()
                    .and_then(|report| report.account_email.clone())
            });
        let refresh_failure = engine_refresh_failure(&self.profile_usage, engine_id);
        let refreshing = self
            .profile_usage
            .refreshing_engine_ids
            .iter()
            .any(|refreshing| refreshing == engine_id);
        let phase = self.catalog_controller.catalog_phase();
        let catalog = match phase {
            NativeCatalogPhase::Ready => SettingsEngineCatalogState::Ready,
            NativeCatalogPhase::Failed => SettingsEngineCatalogState::Failed,
            NativeCatalogPhase::Loading | NativeCatalogPhase::Offline => {
                SettingsEngineCatalogState::Loading
            }
        };
        let catalog_error = if phase == NativeCatalogPhase::Failed {
            Some("Runtime model catalog is unavailable. Retry model loading.".to_owned())
        } else if self.catalog_controller.favorites_failure().is_some() {
            Some(
                "Model favorites could not be synchronized. Retry the favorite action."
                    .to_owned(),
            )
        } else {
            None
        };
        let registry = match self.engine_settings.registry_view() {
            RegistryView::Loading => SettingsEngineRegistryState::Loading,
            RegistryView::Missing => SettingsEngineRegistryState::Missing,
            RegistryView::PresentEmpty => SettingsEngineRegistryState::Empty,
            RegistryView::Present(_) => SettingsEngineRegistryState::Present,
        };
        let selected_thread = self
            .selected_thread
            .as_ref()
            .map(|thread| thread.as_str().to_owned());
        let authoritative = self.engine_settings.authoritative_config();
        let effective = self.effective_catalog_snapshot(cx);
        let saved_policy = authoritative.and_then(|config| {
            if config.selection().engine_id().as_str() != engine_id {
                return None;
            }
            crate::composer_model_config::policy_for_selection(&effective, config).ok()
        });
        let saved_model = saved_policy
            .as_ref()
            .map(|policy| policy.model_id.clone())
            .or_else(|| {
                authoritative.and_then(|config| {
                    (config.selection().engine_id().as_str() == engine_id)
                        .then(|| {
                            config
                                .selection()
                                .model_id()
                                .map(|model| model.as_str().to_owned())
                        })
                        .flatten()
                })
            });
        let saved_profile = authoritative.and_then(|config| {
            (config.selection().engine_id().as_str() == engine_id)
                .then(|| config.selection().profile_id().as_str().to_owned())
        });
        let displayed = self.displayed_policy_for_engine(engine_id, cx);
        let displayed_model = displayed.as_ref().map(|policy| policy.model_id.clone());
        let displayed_authoritative = match (&displayed, authoritative) {
            (Some(displayed), Some(saved)) => {
                crate::composer_model_config::config_for_policy(&effective, displayed, Some(saved))
                    .ok()
                    .as_ref()
                    == Some(saved)
            }
            _ => false,
        };
        let pending_save = self.engine_settings.pending_save_request_id().is_some();
        let save_failed = self.engine_settings.failure_operation()
            == Some(EngineSettingsFailureOperation::Save);
        let can_save_displayed = selected_thread.is_some()
            && !pending_save
            && !displayed_authoritative
            && displayed.as_ref().is_some_and(|policy| {
                crate::composer_model_config::config_for_policy(
                    &effective,
                    policy,
                    authoritative,
                )
                .ok()
                .is_some_and(|config| Some(&config) != authoritative)
            });
        // An explicit choice held without a save names its honest blocker:
        // no thread, or the live admission reason. Saved, saving, and
        // failed states read out their own rows instead.
        let choice_notice = match (&displayed, &selected_thread) {
            (Some(policy), None)
                if self.composer_model_choice.as_ref().is_some_and(
                    |(thread, choice)| {
                        thread == &self.selected_thread && choice.engine_id == engine_id
                    },
                ) =>
            {
                Some(format!(
                    "“{}” is selected. Select a thread to save it.",
                    policy.model_id
                ))
            }
            (Some(policy), Some(_))
                if !pending_save
                    && !save_failed
                    && !displayed_authoritative
                    && self.composer_model_choice.as_ref().is_some_and(
                        |(thread, choice)| {
                            thread == &self.selected_thread && choice.engine_id == engine_id
                        },
                    )
                    && effective.admit_policy(policy).is_err() =>
            {
                Some(self.readiness_block_reason(&policy.engine_id))
            }
            _ => None,
        };
        let saved_id = saved_policy.as_ref().map(|policy| policy.model_id.clone());
        let models = effective
            .manifest
            .models
            .iter()
            .filter(|model| model.harness == engine_id)
            .map(|model| SettingsEngineModel {
                id: model.id.clone(),
                saved: saved_id.as_deref() == Some(model.id.as_str()),
                displayed: displayed_model.as_deref() == Some(model.id.as_str()),
                disabled_reason: model
                    .disabled
                    .as_ref()
                    .map(|disabled| disabled.reason.clone()),
            })
            .collect();
        SettingsEngineSnapshot {
            engine_id: engine_id.to_owned(),
            readiness,
            account_email,
            refresh_failure,
            refreshing,
            catalog,
            catalog_error,
            registry,
            selected_thread,
            saved_model,
            saved_profile,
            displayed_model,
            displayed_authoritative,
            can_save_displayed,
            pending_save,
            save_failed,
            choice_notice,
            models,
        }
    }

    /// Rebuilds the mounted engine snapshot, if an engine page is mounted.
    ///
    /// Called from transport and settings event handlers — never from
    /// render-synced projections — so the page follows acknowledgments,
    /// conflicts, failures, catalog reads, and usage replies.
    fn refresh_settings_engine_snapshot(&mut self, cx: &mut Context<Self>) {
        let (Some(screen), Some((SettingsRoute::Engines, Some(engine_id)))) =
            (self.settings_screen.clone(), self.settings_screen_key.clone())
        else {
            return;
        };
        if engine_id == crate::native_settings::FIXTURE_ENGINE_ID {
            return;
        }
        let snapshot = self.settings_engine_snapshot(&engine_id, cx);
        screen.update(cx, |screen, screen_cx| {
            screen.set_engine_snapshot(snapshot, screen_cx);
        });
    }

    /// Serves one Settings model choice through the shared picker flow.
    ///
    /// The catalog model becomes a `SelectPolicy` on the effective catalog
    /// and travels the existing composer selection path — same admission,
    /// same direct typed save with compare-and-swap, same acknowledgment —
    /// so a Settings choice is durable exactly like a composer one. An
    /// engine mismatch or unknown model is ignored; an unrunnable choice is
    /// stored with the live admission reason.
    fn choose_settings_engine_model(
        &mut self,
        engine_id: &str,
        model_id: &str,
        cx: &mut Context<Self>,
    ) {
        let catalog = self.effective_catalog_snapshot(cx);
        let Ok(raw) = catalog.selection_policy_for_model(model_id) else {
            return;
        };
        if raw.engine_id != engine_id {
            return;
        }
        let policy = crate::composer_model_config::with_default_native_profile(&raw);
        if catalog.admit_policy(&policy).is_err() {
            self.composer_model_run_error = Some(self.readiness_block_reason(&policy.engine_id));
        }
        self.handle_composer_model_event(
            &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(policy),
            cx,
        );
        self.refresh_settings_engine_snapshot(cx);
    }

    /// Saves the Settings-displayed model through the shared direct save.
    fn save_settings_displayed_model(&mut self, engine_id: &str, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        if self.engine_settings.pending_save_request_id().is_some() {
            return;
        }
        let Some(policy) = self.displayed_policy_for_engine(engine_id, cx) else {
            return;
        };
        let catalog = self.effective_catalog_snapshot(cx);
        let Ok(config) = crate::composer_model_config::config_for_policy(
            &catalog,
            &policy,
            self.engine_settings.authoritative_config(),
        ) else {
            self.composer_model_run_error = Some(self.readiness_block_reason(&policy.engine_id));
            self.sync_composer_controls(cx);
            return;
        };
        if Some(&config) == self.engine_settings.authoritative_config() {
            return;
        }
        if !self.submit_direct_save(thread_id, config) {
            self.composer_model_run_error = Some(
                "Engine settings could not be saved. Your draft is preserved; retry the model selection."
                    .to_owned(),
            );
        }
        self.sync_composer_model_policy(cx);
        cx.notify();
    }

    /// Serves one mounted Settings screen action.
    fn handle_settings_screen_event(
        &mut self,
        _screen: Entity<SettingsScreen>,
        event: &SettingsScreenEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SettingsScreenEvent::Navigate { section, engine } => {
                self.navigate(
                    NativeRoute::Settings {
                        section: *section,
                        engine: engine.clone(),
                    },
                    cx,
                );
            }
            SettingsScreenEvent::RefreshEngine { engine_id } => {
                self.ensure_profile_usage(true, Some(engine_id), cx);
                self.refresh_settings_engine_snapshot(cx);
            }
            SettingsScreenEvent::SaveDisplayedModel { engine_id } => {
                self.save_settings_displayed_model(engine_id, cx);
                self.refresh_settings_engine_snapshot(cx);
            }
            SettingsScreenEvent::SelectEngineModel {
                engine_id,
                model_id,
            } => {
                self.choose_settings_engine_model(engine_id, model_id, cx);
            }
        }
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

    /// Submits a first-send configuration save carrying a validated engine
    /// configuration directly. This shares [`Self::submit_direct_save`] with
    /// every native model/effort/profile selection: the settings draft stays
    /// `OpenCode` 2-shaped until per-engine settings UI lands, so native
    /// selections bypass it and travel with the controller-derived
    /// compare-and-swap precondition (`Unconfigured` on an unconfigured
    /// thread) instead. Returns whether the save is now tracked for its
    /// authoritative acknowledgment, which continues the pending first send.
    fn submit_first_send_save(
        &mut self,
        thread_id: ThreadId,
        config: artisan_domain::EngineRunConfig,
    ) -> bool {
        self.submit_direct_save(thread_id, config)
    }

    /// Submits a validated engine configuration through the shared direct
    /// typed-save path, bypassing the `OpenCode` 2-shaped draft.
    ///
    /// The compare-and-swap precondition comes from the controller:
    /// `Unconfigured` for a first send, `Exact` on the authoritative
    /// revision for a later model/effort/profile change, so a concurrent
    /// writer conflicts instead of being silently overwritten. Pending-send
    /// safety is unchanged: the save is tracked for its real acknowledgment,
    /// which continues or fails the held send. Returns whether the save is
    /// now tracked for its authoritative acknowledgment.
    fn submit_direct_save(
        &mut self,
        thread_id: ThreadId,
        config: artisan_domain::EngineRunConfig,
    ) -> bool {
        // The controller selection follows the mounted thread; selecting
        // again is a no-op when it already does.
        self.engine_settings.select_thread(Some(&thread_id));
        let request_id = match create_save_request_id() {
            Ok(id) => id,
            Err(failure) => {
                self.engine_settings.on_save_admission_failed(failure);
                return false;
            }
        };
        let Some(command) = self
            .engine_settings
            .build_direct_save_command(request_id.clone(), config.clone())
        else {
            return false;
        };
        if !self
            .engine_settings
            .begin_direct_save(thread_id, request_id, config)
        {
            return false;
        }
        match self.submit_command(NativeTransportCommand::SetThreadEngineConfig(Box::new(
            command,
        ))) {
            Ok(()) => true,
            Err(error) => {
                self.engine_settings
                    .on_save_admission_failed(command_failure(error));
                false
            }
        }
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
        let sidebar = self.desktop_sidebar(window, cx).into_any_element();
        let body = self.desktop_route_body(window, cx);
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
    fn route_surface(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
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
                    // The environment card must never infer a disconnect from
                    // its empty default: feed the readily existing profile
                    // hostname (the same authoritative identity behind the
                    // sidebar) so Machine names this computer instead of
                    // reporting `Not connected` while connected.
                    let environment = ThreadEnvironmentInput {
                        identity: self.profile_hostname.clone().map(HostIdentitySnapshot::new),
                        ..ThreadEnvironmentInput::default()
                    };
                    let mounted = match self.conversation_host.clone() {
                        Some(host) => {
                            let composer = self.composer.clone();
                            let screen = cx.new(|screen_cx| {
                                ThreadScreen::new(host, composer, ThemeMode::Dark, screen_cx)
                            });
                            screen.update(cx, |screen, _| {
                                screen.set_gate(ThreadScreenGate::Open);
                                screen.set_environment(environment.clone());
                            });
                            // Typed text reaches the composer only while it
                            // holds window focus (the platform registers its
                            // text handler for the focused handle, and
                            // unfocused keystrokes are silently swallowed).
                            // Focus it once per opened screen so a fresh
                            // thread is immediately typeable; later renders
                            // must not steal focus back.
                            let focus = self.composer.read(cx).focus_handle(cx);
                            window.focus(&focus, cx);
                            Some(screen)
                        }
                        None => ThreadScreen::mount(thread.clone(), ThemeMode::Dark, cx)
                            .ok()
                            .map(|screen| {
                                screen.update(cx, |screen, _| {
                                    screen.set_environment(environment);
                                });
                                screen
                            }),
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
                        SettingsScreen::new(section, engine.clone(), ThemeMode::Dark, screen_cx)
                    });
                    // The rail enumerates the manifest harness identities so
                    // every real catalog engine is reachable from the normal
                    // Models entry point; fixture identities never enter
                    // production navigation.
                    let entries = self
                        .model_selector
                        .read(cx)
                        .state()
                        .snapshot()
                        .manifest
                        .harnesses
                        .iter()
                        .map(|harness| SettingsEngineNavEntry {
                            id: harness.id.clone(),
                            label: harness.label.clone(),
                        })
                        .collect::<Vec<_>>();
                    screen.update(cx, |screen, screen_cx| {
                        screen.set_engines(entries, screen_cx);
                    });
                    // A real catalog engine mounts its live page; anything
                    // else keeps the legacy surface (including the fixture
                    // route, which stays out of the rail above).
                    if section == SettingsRoute::Engines
                        && let Some(engine_id) = engine.clone()
                        && engine_id != crate::native_settings::FIXTURE_ENGINE_ID
                    {
                        let snapshot = self.settings_engine_snapshot(&engine_id, cx);
                        let known = self
                            .model_selector
                            .read(cx)
                            .state()
                            .snapshot()
                            .manifest
                            .harness(&engine_id)
                            .is_some();
                        let label = self
                            .model_selector
                            .read(cx)
                            .state()
                            .snapshot()
                            .manifest
                            .harness(&engine_id)
                            .map(|harness| harness.label.clone());
                        screen.update(cx, |screen, screen_cx| {
                            screen.set_engine_known(known, screen_cx);
                            screen.set_engine_label(label, screen_cx);
                            if known {
                                screen.set_engine_snapshot(snapshot, screen_cx);
                            }
                        });
                    }
                    let subscription =
                        cx.subscribe(&screen, Self::handle_settings_screen_event);
                    self.settings_screen_subscription = Some(subscription);
                    self.settings_screen = Some(screen);
                    self.settings_screen_key = key;
                }
                self.settings_screen
                    .clone()
                    .expect("settings screen mounted")
                    .into_any_element()
            }
            NativeRoute::NewThread { .. } => self
                .new_thread_surface_section(window, cx)
                .into_any_element(),
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

fn profile_usage_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// Display a raw OS account or machine string with only its first letter
/// capitalized, so `sander` paints as `Sander`. The stored value is left
/// untouched so avatar seeds and identity matching stay stable.
fn capitalize_label(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        None => String::new(),
        Some(first) => {
            first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
        }
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
        DESKTOP_COMPOSER_SELECTOR, DESKTOP_HOME_SELECTOR, DESKTOP_OFFLINE_SELECTOR,
        DESKTOP_SIDEBAR_SELECTOR, DESKTOP_TITLEBAR_SELECTOR,
    };
    use crate::native_command_menu::{
        COMMAND_MENU_DROPDOWN_SELECTOR, COMMAND_MENU_INPUT_SELECTOR, COMMAND_MENU_LIST_SELECTOR,
    };
    use crate::native_profile_usage::{
        NativeUsageAuthentication, NativeUsageCadence, NativeUsageEntry, NativeUsageQuotaSurface,
        NativeUsageReport, NativeUsageWindow,
    };
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
        project_picker::{PickerRow, ProjectOption, ProjectPickerAction},
    };
    use artisan_domain::{
        ConversationCursor, ConversationSnapshot, ConversationSubscriptionStart, DisplayName,
        ObservationId, ProjectId, ProjectListing, ProjectSummary, ReceiptDisposition, RequestId,
        RootPath, RunId, ThreadId, ThreadListing, ThreadSummary, ThreadTitle, UnixMillis,
    };
    use artisan_protocol::{
        ConversationSubscriptionStarted, ConversationSubscriptionStopped, QueueMessageReceipt,
    };
    use artisan_ui::button::{
        Button, ButtonContent, ButtonSize, ButtonStyle, ButtonVariant, FocusVisibility,
    };
    use artisan_ui::motion::MotionPolicy;
    use artisan_ui::theme::{ArtisanTheme, ThemeMode};
    use gpui::{
        Context, Focusable as _, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext,
    };
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

    fn reported_usage_window(
        id: &str,
        cadence: NativeUsageCadence,
        label: Option<&str>,
        percent_used: f64,
    ) -> NativeUsageWindow {
        NativeUsageWindow {
            id: id.to_owned(),
            cadence,
            label: label.map(str::to_owned),
            percent_used,
            resets_at: None,
            window_minutes: None,
        }
    }

    fn reported_usage_entry(
        engine_id: &str,
        display_name: &str,
        windows: Vec<NativeUsageWindow>,
    ) -> NativeUsageEntry {
        reported_usage_entry_with_auth(
            engine_id,
            display_name,
            NativeUsageAuthentication::Authenticated,
            windows,
        )
    }

    fn reported_usage_entry_with_auth(
        engine_id: &str,
        display_name: &str,
        authentication: NativeUsageAuthentication,
        windows: Vec<NativeUsageWindow>,
    ) -> NativeUsageEntry {
        NativeUsageEntry {
            engine_id: engine_id.to_owned(),
            display_name: display_name.to_owned(),
            report: Some(NativeUsageReport {
                engine_id: engine_id.to_owned(),
                display_name: display_name.to_owned(),
                authentication,
                account_email: None,
                quota_surface: NativeUsageQuotaSurface::Supported,
                windows,
                failure: None,
            }),
            failure: None,
            fetched_at_ms: Some(super::profile_usage_now_ms().saturating_sub(60_000)),
        }
    }

    /// Tall real-data fixture: eight authenticated providers with three
    /// cadence windows each (one at exactly zero, which stays visible),
    /// replacing the old pending placeholders without weakening scroll
    /// assertions.
    fn install_connected_profile_usage(
        application: &mut NativeApplication,
        sink: NativeTestCommandSink,
    ) {
        application.test_command_sink = Some(sink);
        let engines = [
            "profile-test-alpha",
            "profile-test-beta",
            "profile-test-gamma",
            "profile-test-delta",
            "profile-test-epsilon",
            "profile-test-zeta",
            "profile-test-eta",
            "profile-test-theta",
        ];
        for (index, engine_id) in engines.iter().enumerate() {
            let base = (index * 11) as f64;
            application.profile_usage.entries.push(reported_usage_entry(
                engine_id,
                &format!("Profile Test {index}"),
                vec![
                    reported_usage_window(
                        "session",
                        NativeUsageCadence::Session,
                        None,
                        if index == 0 { 0.0 } else { 10.0 + base },
                    ),
                    reported_usage_window(
                        "weekly-model",
                        NativeUsageCadence::Weekly,
                        Some("Model"),
                        20.0 + base,
                    ),
                    reported_usage_window(
                        "monthly",
                        NativeUsageCadence::Monthly,
                        None,
                        30.0 + base,
                    ),
                ],
            ));
        }
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

    /// Drives the engine-settings controller to a persisted configuration
    /// built from the offline `codex-sol` policy, so send-admission tests
    /// exercise the configured first-send flow instead of the unconfigured
    /// block. Uses only controller-local transitions; no transport.
    fn install_configured_engine_settings(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
    ) {
        let thread_id = application
            .selected_thread
            .clone()
            .expect("selected settings thread");
        let catalog = application.model_selector.read(cx).state().snapshot().clone();
        let mut policy = catalog
            .selection_policy_for_model("codex-sol")
            .expect("default selection policy");
        policy.profile_id = Some("default".to_owned());
        let config =
            crate::composer_model_config::config_for_policy(&catalog, &policy, None)
                .expect("default policy builds a run configuration");
        application.engine_settings.select_thread(Some(&thread_id));
        let generation = application
            .engine_settings
            .prepare_settings_load()
            .expect("settings load generation");
        assert!(application.engine_settings.mark_settings_load_admitted(&thread_id, generation));
        application.engine_settings.on_settings_loaded(
            generation,
            artisan_protocol::ThreadEngineSettingsResult::Configured {
                thread_id,
                revision: artisan_domain::EngineConfigRevision::new(1).expect("first revision"),
                config: Box::new(config),
            },
        );
        assert!(application.engine_settings.authoritative_config().is_some());
    }

    /// Admits a fresh authenticated Codex usage reply through the real
    /// request and response handlers.
    ///
    /// Admission observes probed readiness exactly like production instead
    /// of a manually seated runnable flag: the usage read is dispatched
    /// through the freshness planner and the reply settles through the
    /// generation/sequence pairing.
    fn admit_probed_codex_usage(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
    ) {
        application.ensure_profile_usage(false, Some("codex"), cx);
        let generation = application.profile_usage_generation;
        let request_seq = application
            .profile_usage
            .pending_seq("codex")
            .expect("codex usage read admitted");
        application.handle_account_usage(
            "codex".to_owned(),
            generation,
            request_seq,
            reported_usage_entry("codex", "Codex", Vec::new()),
            cx,
        );
    }

    /// Returns the request identity of the currently tracked save.
    fn admitted_save_request(application: &NativeApplication) -> RequestId {
        application
            .engine_settings
            .pending_save_request_id()
            .cloned()
            .expect("admitted save")
    }

    /// Drives the real first-send admission leg without transport: the real
    /// selection event saves the displayed Codex policy through the shared
    /// direct typed save, and the real send admission holds the flight for
    /// its authoritative acknowledgment. Readiness comes from a usage reply
    /// through the real handler — never from a manually seated runnable
    /// flag or a manually held save. Returns the retained configuration so
    /// tests can acknowledge exactly it.
    fn install_admitted_first_send(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
        thread_id: ThreadId,
        draft: &str,
        sink: NativeTestCommandSink,
    ) -> artisan_domain::EngineRunConfig {
        install_ready_message_surface(application, cx, thread_id.clone(), draft, sink);
        admit_probed_codex_usage(application, cx);
        let policy = application
            .model_selector
            .read(cx)
            .state()
            .snapshot()
            .selection_policy_for_model("codex-sol")
            .expect("codex policy");
        application.handle_composer_model_event(
            &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(policy),
            cx,
        );
        // The selection auto-saves through the shared direct typed-save
        // path; the sink records the exact command production would send.
        let (pending_thread, retained) = application
            .engine_settings
            .pending_save()
            .map(|(thread, config)| (thread.clone(), config.clone()))
            .expect("selection save admitted");
        assert_eq!(pending_thread, thread_id);
        application.begin_message_submission(cx);
        assert!(application.pending_first_send.is_some());
        assert!(application.composer.read(cx).is_submitting());
        retained
    }

    fn answer_thread() -> ThreadId {
        ThreadId::parse("thread-answer").expect("answer thread")
    }

    fn answer_run() -> RunId {
        RunId::parse("run-answer").expect("answer run")
    }

    fn answer_approval() -> ObservationId {
        ObservationId::parse("approval-1").expect("answer approval")
    }

    fn install_answer_surface(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
        sink: NativeTestCommandSink,
    ) {
        let thread_id = answer_thread();
        let host = ConversationHost::mount(thread_id.clone(), ThemeMode::Dark, &mut *cx)
            .expect("answer host");
        application.selected_thread = Some(thread_id);
        application.conversation_host = Some(host);
        application.test_command_sink = Some(sink);
    }

    fn queue_approval_answer(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
    ) -> RequestId {
        let host = application.conversation_host.clone().expect("answer host");
        let surface = host.read(cx).surface().clone();
        let request_id = surface.update(cx, |surface, surface_cx| {
            surface.set_answer_context(answer_thread(), answer_run(), surface_cx);
            assert!(surface.submit_approval_gesture(
                "approval-1",
                &answer_approval(),
                true,
                surface_cx,
            ));
            surface.pending_answer_dispatches()[0].request_id.clone()
        });
        request_id
    }

    fn recorded_approval(commands: &[NativeTransportCommand]) -> &artisan_domain::RespondApproval {
        assert_eq!(commands.len(), 1);
        if let NativeTransportCommand::RespondApproval(answer) = &commands[0] {
            answer
        } else {
            panic!("tick must submit an approval answer")
        }
    }

    #[gpui::test]
    fn tick_drains_queued_answer_into_submit_with_preserved_ids(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([]);
        let expected = cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_answer_surface(application, cx, sink);
                let expected = queue_approval_answer(application, cx);
                application.poll_service(cx);
                expected
            })
        });
        let recorded = commands.borrow();
        let submitted = recorded_approval(&recorded);
        assert_eq!(submitted.request_id(), &expected);
        assert_eq!(submitted.thread_id(), &answer_thread());
        assert_eq!(submitted.run_id(), &answer_run());
        assert_eq!(submitted.approval_id().as_str(), "approval-1");
        assert!(submitted.approved);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                let host = application.conversation_host.clone().expect("answer host");
                assert!(
                    host.read(cx)
                        .surface()
                        .read(cx)
                        .pending_answer_dispatches()
                        .is_empty()
                );
            });
        });
    }

    #[gpui::test]
    fn tick_busy_keeps_row_pending_with_retry_state(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Err(super::CommandSendError::Busy)]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_answer_surface(application, cx, sink);
                let expected = queue_approval_answer(application, cx);
                application.poll_service(cx);
                assert_eq!(commands.borrow().len(), 1);
                let host = application.conversation_host.clone().expect("answer host");
                let surface = host.read(cx).surface().clone();
                assert_eq!(surface.read(cx).pending_answer_dispatches().len(), 1);
                assert_eq!(
                    surface.read(cx).pending_answer_dispatches()[0].request_id,
                    expected,
                    "nothing is silently dropped"
                );
                assert!(
                    !surface.update(cx, |surface, surface_cx| {
                        surface.submit_approval_gesture(
                            "approval-1",
                            &answer_approval(),
                            true,
                            surface_cx,
                        )
                    }),
                    "single-flight holds across ticks until receipt pairing"
                );
            });
        });
    }

    #[gpui::test]
    fn tick_stopped_reports_diagnostic_without_drop(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Err(super::CommandSendError::Stopped)]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_answer_surface(application, cx, sink);
                queue_approval_answer(application, cx);
                application.poll_service(cx);
                assert_eq!(commands.borrow().len(), 1);
                let host = application.conversation_host.clone().expect("answer host");
                assert_eq!(
                    host.read(cx)
                        .surface()
                        .read(cx)
                        .pending_answer_dispatches()
                        .len(),
                    1,
                    "a stopped service degrades without drop"
                );
            });
        });
    }

    #[gpui::test]
    fn tick_empty_outbox_leaves_transport_untouched(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Err(super::CommandSendError::Busy)]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_answer_surface(application, cx, sink);
                application.poll_service(cx);
                assert!(commands.borrow().is_empty());
            });
        });
    }

    #[gpui::test]
    fn second_tick_does_not_resend_before_pairing(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_answer_surface(application, cx, sink);
                queue_approval_answer(application, cx);
                application.poll_service(cx);
                application.poll_service(cx);
                assert_eq!(commands.borrow().len(), 1);
            });
        });
    }

    #[gpui::test]
    fn picker_offline_choice_survives_sync_and_rejects_send_without_losing_draft(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_ready_message_surface(
                    application,
                    cx,
                    ThreadId::parse("picker-task").unwrap(),
                    "keep my draft",
                    sink,
                );
                let policy = application
                    .model_selector
                    .read(cx)
                    .state()
                    .snapshot()
                    .selection_policy_for_model("codex-sol")
                    .unwrap();
                application.handle_composer_model_event(
                    &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(
                        policy.clone(),
                    ),
                    cx,
                );
                application.sync_composer_model_policy(cx);
                assert_eq!(
                    application.model_selector.read(cx).state().policy(),
                    Some(&policy)
                );
                assert!(
                    application
                        .model_selector
                        .read(cx)
                        .state()
                        .status()
                        .error
                        .is_none()
                );
                assert!(application.composer_model_run_error.is_none());
                application.begin_message_submission(cx);
                assert!(commands.borrow().is_empty());
                assert_eq!(application.composer.read(cx).draft(), "keep my draft");
                assert!(!application.composer.read(cx).is_submitting());
                assert!(application.composer_model_run_error.is_some());
                application.selected_thread = None;
                application.sync_composer_model_policy(cx);
                assert!(application.composer_model_choice.is_none());
                assert!(application.composer_model_run_error.is_none());
            });
        });
    }

    #[gpui::test]
    fn unconfigured_first_send_with_displayed_policy_blocks_and_preserves_draft(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_ready_message_surface(
                    application,
                    cx,
                    ThreadId::parse("first-send-task").unwrap(),
                    "keep my draft",
                    sink,
                );
                assert!(application.engine_settings.authoritative_config().is_none());
                // The picker always displays its default policy, exactly the
                // production first-send shape: a visible model with no
                // persisted thread configuration and no explicit choice.
                assert!(application.composer_model_choice.is_none());
                assert!(application.model_selector.read(cx).state().policy().is_some());
                application.begin_message_submission(cx);
                // Blocked with the live verdict: admission requests the
                // backend-probed account check first, so the first send
                // observes a pending check rather than a catch-all. The
                // only commands are those account reads; no save, no
                // flight, draft preserved.
                let recorded = commands.borrow();
                assert_eq!(recorded.len(), 6);
                assert!(recorded.iter().all(|command| matches!(
                    command,
                    NativeTransportCommand::ReadAccountUsage { .. }
                )));
                assert!(application.message_flight.is_none());
                assert!(!application.composer.read(cx).is_submitting());
                assert_eq!(application.composer.read(cx).draft(), "keep my draft");
                let error = application
                    .composer_model_run_error
                    .clone()
                    .expect("first-send configuration error");
                assert!(
                    error.contains("account status") && error.contains("Your draft is preserved"),
                    "unexpected error: {error}"
                );
            });
        });
    }

    #[gpui::test]
    fn explicit_policy_selection_reaches_save_and_sends_on_ack(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("explicit-first-send-task").expect("thread");
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_ready_message_surface(
                    application,
                    cx,
                    thread_id.clone(),
                    "explicit pick draft",
                    sink,
                );
                // Readiness arrives as a probed usage reply through the real
                // handler — never as a manually seated runnable flag — so
                // the explicit choice below travels the production path.
                admit_probed_codex_usage(application, cx);
                // The real selection event with the picker's own policy
                // shape: no validate-run gate may strand this explicit
                // choice before the first-send save flow, and no manual
                // profile is needed for the native default.
                let policy = application
                    .model_selector
                    .read(cx)
                    .state()
                    .snapshot()
                    .selection_policy_for_model("codex-sol")
                    .expect("codex policy");
                application.handle_composer_model_event(
                    &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(
                        policy,
                    ),
                    cx,
                );
                assert!(application.engine_settings.authoritative_config().is_none());
                // The selection auto-saved through the shared direct typed
                // save; the send adopts that in-flight save and holds for
                // its acknowledgment.
                let save_request = admitted_save_request(application);
                let retained = application
                    .engine_settings
                    .pending_save()
                    .map(|(_, config)| config.clone())
                    .expect("selection save tracked");
                application.begin_message_submission(cx);
                assert!(application.pending_first_send.is_some());
                assert!(application.composer_model_run_error.is_some());
                application.handle_engine_config_set(
                    &artisan_protocol::SetThreadEngineConfigResult {
                        request_id: save_request,
                        thread_id: thread_id.clone(),
                        revision: artisan_domain::EngineConfigRevision::new(1)
                            .expect("revision"),
                        disposition: artisan_domain::ReceiptDisposition::Accepted,
                    },
                    retained,
                    cx,
                );
                let flight = application
                    .message_flight
                    .as_ref()
                    .expect("continued flight");
                assert_eq!(flight.thread_id, thread_id);
                assert_eq!(
                    flight.payload.text().expect("text payload").as_str(),
                    "explicit pick draft"
                );
                assert!(application.pending_first_send.is_none());
                assert!(application.composer_model_run_error.is_none());
            });
        });
        let commands = commands.borrow();
        // The production-shaped command sequence: the probed account read,
        // the typed selection save with its `Unconfigured` precondition,
        // then the continued queue after the authoritative acknowledgment.
        // Nothing here seats readiness or holds the save by hand.
        let save = commands
            .iter()
            .find_map(|command| match command {
                NativeTransportCommand::SetThreadEngineConfig(command) => Some(command),
                _ => None,
            })
            .expect("typed selection save");
        assert_eq!(save.thread_id(), &thread_id);
        assert_eq!(
            save.precondition(),
            artisan_domain::EngineConfigUpdatePrecondition::Unconfigured
        );
        let queued = commands
            .iter()
            .find_map(|command| match command {
                NativeTransportCommand::QueueMessage(command) => Some(command),
                _ => None,
            })
            .expect("acknowledged explicit send must queue its message");
        assert_eq!(queued.thread_id, thread_id);
        assert_eq!(
            queued.payload.text().expect("text payload").as_str(),
            "explicit pick draft"
        );
    }

    /// Navigates to one engine Settings page.
    fn mount_settings_engine(
        application: &mut NativeApplication,
        cx: &mut Context<NativeApplication>,
        engine_id: &str,
    ) {
        application.navigate(
            NativeRoute::Settings {
                section: SettingsRoute::Engines,
                engine: Some(engine_id.to_owned()),
            },
            cx,
        );
    }

    #[gpui::test]
    fn settings_rail_lists_real_engines_without_a_thread(cx: &mut TestAppContext) {        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                application.test_command_sink = Some(sink);
                // The normal entry point from the profile menu: the Models
                // section with no engine and no selected thread.
                application.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::Models,
                        engine: None,
                    },
                    cx,
                );
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                let screen = application
                    .settings_screen
                    .clone()
                    .expect("settings screen mounted");
                let ids: Vec<String> = screen
                    .read(cx)
                    .engines()
                    .iter()
                    .map(|entry| entry.id.clone())
                    .collect();
                // Every real catalog engine is reachable; the mock fixture
                // identity never enters production navigation.
                for expected in ["codex", "claude", "cursor", "grok", "hermes", "opencode2"] {
                    assert!(
                        ids.iter().any(|id| id == expected),
                        "rail must enumerate {expected}: {ids:?}"
                    );
                }
                assert!(
                    !ids.iter().any(|id| id == "fixture-engine"),
                    "rail must not list fixture identities: {ids:?}"
                );
                // Entering Settings requested the global readiness refresh
                // even with no thread selected.
                assert!(
                    commands.borrow().iter().any(|command| matches!(
                        command,
                        NativeTransportCommand::ReadAccountUsage { .. }
                    )),
                    "settings entry must refresh account readiness"
                );
            });
        });
        // The real engine nav button routes through its click handler to
        // the live engine page, not a directly invoked navigate call.
        let engine_nav = cx
            .debug_bounds("settings-nav-engines-codex")
            .expect("engine nav mounted");
        cx.simulate_click(engine_nav.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |application, _| {
                assert!(matches!(
                    application.route(),
                    NativeRoute::Settings {
                        section: SettingsRoute::Engines,
                        engine: Some(engine),
                    } if engine == "codex"
                ));
            });
        });
    }

    #[gpui::test]
    fn settings_model_choice_saves_acknowledges_and_reloads(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("settings-choice-task").expect("thread");
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_ready_message_surface(
                    application,
                    cx,
                    thread_id.clone(),
                    "settings draft",
                    sink,
                );
                admit_probed_codex_usage(application, cx);
                mount_settings_engine(application, cx, "codex");
            });
        });
        cx.run_until_parked();
        // The mounted choice travels the shared SelectPolicy plus
        // typed-save flow through the real model-row button handler, not
        // a Settings-only bypass or a directly emitted screen event.
        // Delivery is deferred through the effect queue, so the save is
        // asserted after the click and a parked flush.
        let model_row = cx
            .debug_bounds("settings-engine-model-codex-sol")
            .expect("settings model row mounted");
        cx.simulate_click(model_row.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                let save_request = admitted_save_request(application);
                let retained = application
                    .engine_settings
                    .pending_save()
                    .map(|(_, config)| config.clone())
                    .expect("settings choice save tracked");
                application.handle_engine_config_set(
                    &artisan_protocol::SetThreadEngineConfigResult {
                        request_id: save_request,
                        thread_id: thread_id.clone(),
                        revision: artisan_domain::EngineConfigRevision::new(1)
                            .expect("revision"),
                        disposition: artisan_domain::ReceiptDisposition::Accepted,
                    },
                    retained.clone(),
                    cx,
                );
                assert!(application.engine_settings.authoritative_config().is_some());
            });
        });
        // The typed save went out with the first-send precondition…
        let save = commands
            .borrow()
            .iter()
            .find_map(|command| match command {
                NativeTransportCommand::SetThreadEngineConfig(command) => Some(command.clone()),
                _ => None,
            })
            .expect("settings choice save");
        assert_eq!(
            save.precondition(),
            artisan_domain::EngineConfigUpdatePrecondition::Unconfigured
        );
        // …and reopening the thread restores the saved Codex model from
        // durable storage instead of the cleared in-memory choice.
        // `select_thread(None)` clears the authoritative config, so the
        // saved config is preserved first for the reload reply.
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                let saved = application
                    .engine_settings
                    .authoritative_config()
                    .cloned()
                    .expect("saved configuration");
                application.composer_model_choice = None;
                application.engine_settings.select_thread(None);
                application.engine_settings.select_thread(Some(&thread_id));
                application.submit_settings_load(thread_id.clone());
                let generation = application
                    .engine_settings
                    .active_settings_generation()
                    .expect("settings load admitted");
                application.handle_engine_settings(
                    generation,
                    artisan_protocol::ThreadEngineSettingsResult::Configured {
                        thread_id: thread_id.clone(),
                        revision: artisan_domain::EngineConfigRevision::new(1)
                            .expect("revision"),
                        config: Box::new(saved),
                    },
                    cx,
                );
                let policy = application
                    .model_selector
                    .read(cx)
                    .state()
                    .policy()
                    .cloned()
                    .expect("reloaded policy");
                assert_eq!(policy.model_id, "codex-sol");
                assert_eq!(policy.profile_id.as_deref(), Some("default"));
                assert!(
                    application.model_selector.read(cx).state().status().authoritative
                );
                let screen = application
                    .settings_screen
                    .clone()
                    .expect("settings screen mounted");
                let snapshot = screen
                    .read(cx)
                    .engine_snapshot()
                    .cloned()
                    .expect("engine snapshot");
                assert_eq!(snapshot.saved_model.as_deref(), Some("codex-sol"));
                assert!(snapshot.models.iter().any(|row| row.id == "codex-sol" && row.saved));
            });
        });
    }

    #[gpui::test]
    fn signed_out_refresh_removes_admission_and_updates_settings(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("settings-signout-task").expect("thread");
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_ready_message_surface(
                    application,
                    cx,
                    thread_id.clone(),
                    "draft",
                    sink,
                );
                admit_probed_codex_usage(application, cx);
                assert!(
                    application
                        .effective_catalog_snapshot(cx)
                        .selectability("codex-sol")
                        .is_available()
                );
                mount_settings_engine(application, cx, "codex");
            });
        });
        cx.run_until_parked();
        // The mounted Settings refresh button forces a probed re-read
        // through the real transport command; the reply is fed after the
        // click delivery flushes.
        let refresh = cx
            .debug_bounds("settings-installation-refresh")
            .expect("settings refresh mounted");
        cx.simulate_click(refresh.center(), gpui::Modifiers::default());
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                let forced = commands.borrow();
                assert!(
                    forced.iter().any(|command| matches!(
                        command,
                        NativeTransportCommand::ReadAccountUsage { force: true, .. }
                    )),
                    "refresh action must force an account re-read"
                );
                drop(forced);
                // The signed-out reply removes the admission and updates the
                // mounted page through the real response handler.
                let generation = application.profile_usage_generation;
                let request_seq = application
                    .profile_usage
                    .pending_seq("codex")
                    .expect("forced codex re-read admitted");
                application.handle_account_usage(
                    "codex".to_owned(),
                    generation,
                    request_seq,
                    reported_usage_entry_with_auth(
                        "codex",
                        "Codex",
                        NativeUsageAuthentication::Unauthenticated,
                        Vec::new(),
                    ),
                    cx,
                );
                assert!(
                    !application
                        .effective_catalog_snapshot(cx)
                        .selectability("codex-sol")
                        .is_available()
                );
                let screen = application
                    .settings_screen
                    .clone()
                    .expect("settings screen mounted");
                let snapshot = screen
                    .read(cx)
                    .engine_snapshot()
                    .cloned()
                    .expect("engine snapshot");
                assert_eq!(
                    snapshot.readiness,
                    crate::native_profile_usage::EngineReadiness::NeedsSignIn
                );
                assert_ne!(snapshot.saved_model.as_deref(), Some("codex-sol"));
            });
        });
    }

    #[gpui::test]
    fn queued_row_dispatch_error_is_shown_next_to_the_count(
        cx: &mut TestAppContext,
    ) {
        let thread_id = ThreadId::parse("queued-unconfigured-task").expect("thread");
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, _) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_ready_message_surface(
                    application,
                    cx,
                    thread_id.clone(),
                    "draft",
                    sink,
                );
                assert!(application.engine_settings.authoritative_config().is_none());
                // Seed one authoritative queued row, the never-claimed
                // projection the dispatcher still retries.
                application
                    .composer_queue
                    .state
                    .set_scope(Some(thread_id.clone()), 1);
                // The install already forced one listing; retire its refresh
                // so this test owns the next exact refresh token.
                application.composer_queue.state.cancel_queue_refresh();
                let token = application
                    .composer_queue
                    .state
                    .begin_queue_refresh(true, true, false, false, true)
                    .expect("forced queue refresh");
                let listing = artisan_domain::QueuedMessageListing::new(
                    thread_id.clone(),
                    artisan_domain::QueuedMessageListOrder::OldestFirst,
                    1,
                    1,
                    vec![artisan_domain::QueuedMessageSummary {
                        message_id: artisan_domain::MessageId::parse("message-a")
                            .expect("message"),
                        thread_id: thread_id.clone(),
                        original_request_id: artisan_domain::RequestId::parse("command-a")
                            .expect("request"),
                        text: Some(
                            artisan_domain::AuthoredText::parse("queued text").expect("text"),
                        ),
                        attachments: Vec::new(),
                        accepted_at: artisan_domain::UnixMillis::EPOCH,
                        last_error: Some(
                            artisan_domain::DispatchError::parse("engine unconfigured".to_owned())
                                .expect("dispatcher diagnostic"),
                        ),
                    }],
                )
                .expect("queued page");
                application
                    .composer_queue
                    .state
                    .apply_queue_listing(&token, listing)
                    .expect("queue page");
                application.sync_composer_controls(cx);
                let snapshot = application.composer_controls.read(cx).snapshot().clone();
                assert!(!snapshot.run_active);
                let status = snapshot.queue_status.expect("queue status");
                assert!(status.contains("1 queued"), "unexpected status: {status}");
                assert!(status.contains("engine unconfigured"), "unexpected status: {status}");
            });
        });
    }

    #[gpui::test]
    fn save_ack_continues_the_pending_first_send(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("first-send-task").expect("thread");
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        let (retained, save_request) = cx.update(|_, app| {
            view.update(app, |application, cx| {
                let retained = install_admitted_first_send(
                    application,
                    cx,
                    thread_id.clone(),
                    "keep my draft",
                    sink,
                );
                let save_request = admitted_save_request(application);
                (retained, save_request)
            })
        });
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                application.handle_engine_config_set(
                    &artisan_protocol::SetThreadEngineConfigResult {
                        request_id: save_request,
                        thread_id: thread_id.clone(),
                        revision: artisan_domain::EngineConfigRevision::new(1)
                            .expect("revision"),
                        disposition: artisan_domain::ReceiptDisposition::Accepted,
                    },
                    retained,
                    cx,
                );
                let flight = application
                    .message_flight
                    .as_ref()
                    .expect("continued flight");
                assert_eq!(flight.thread_id, thread_id);
                assert_eq!(
                    flight.payload.text().expect("text payload").as_str(),
                    "keep my draft"
                );
                assert!(application.pending_first_send.is_none());
                assert!(application.composer_model_run_error.is_none());
                assert_eq!(application.composer.read(cx).draft(), "keep my draft");
            });
        });
        let commands = commands.borrow();
        // The production-shaped command sequence: the probed account read,
        // the typed first-send save with its `Unconfigured` precondition,
        // then the continued queue after the authoritative acknowledgment.
        let save = commands
            .iter()
            .find_map(|command| match command {
                NativeTransportCommand::SetThreadEngineConfig(command) => Some(command),
                _ => None,
            })
            .expect("typed first-send save");
        assert_eq!(save.thread_id(), &thread_id);
        assert_eq!(
            save.precondition(),
            artisan_domain::EngineConfigUpdatePrecondition::Unconfigured
        );
        let queued = commands
            .iter()
            .find_map(|command| match command {
                NativeTransportCommand::QueueMessage(command) => Some(command),
                _ => None,
            })
            .expect("acknowledged first send must queue its message");
        assert_eq!(queued.thread_id, thread_id);
        assert_eq!(
            queued.payload.text().expect("text payload").as_str(),
            "keep my draft"
        );
    }

    #[gpui::test]
    fn edited_draft_suppresses_the_pending_first_send(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("first-send-edited-task").expect("thread");
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        let (retained, save_request) = cx.update(|_, app| {
            view.update(app, |application, cx| {
                let retained = install_admitted_first_send(
                    application,
                    cx,
                    thread_id.clone(),
                    "original send draft",
                    sink,
                );
                let save_request = admitted_save_request(application);
                (retained, save_request)
            })
        });
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                application.composer.update(cx, |composer, composer_cx| {
                    composer.set_draft("edited before ack");
                    composer_cx.notify();
                });
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                application.handle_engine_config_set(
                    &artisan_protocol::SetThreadEngineConfigResult {
                        request_id: save_request,
                        thread_id: thread_id.clone(),
                        revision: artisan_domain::EngineConfigRevision::new(1)
                            .expect("revision"),
                        disposition: artisan_domain::ReceiptDisposition::Accepted,
                    },
                    retained,
                    cx,
                );
                assert!(application.message_flight.is_none());
                assert!(application.pending_first_send.is_none());
                assert!(!application.composer.read(cx).is_submitting());
                assert_eq!(
                    application.composer.read(cx).draft(),
                    "edited before ack"
                );
                assert!(application.composer_model_run_error.is_none());
            });
        });
        assert!(commands.borrow().is_empty());
    }

    #[gpui::test]
    fn save_failure_suppresses_the_pending_first_send(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("first-send-failed-task").expect("thread");
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_admitted_first_send(
                    application,
                    cx,
                    thread_id.clone(),
                    "keep my draft",
                    sink,
                );
                let save_request = admitted_save_request(application);
                application.handle_engine_config_failed(
                    &thread_id,
                    &save_request,
                    message_failure(),
                    cx,
                );
                assert!(application.message_flight.is_none());
                assert!(application.pending_first_send.is_none());
                assert!(!application.composer.read(cx).is_submitting());
                assert_eq!(application.composer.read(cx).draft(), "keep my draft");
                let error = application
                    .composer_model_run_error
                    .clone()
                    .expect("save failure error");
                assert!(error.contains("preserved"), "unexpected error: {error}");
            });
        });
        assert!(commands.borrow().is_empty());
    }

    #[gpui::test]
    fn thread_switch_suppresses_the_pending_first_send(cx: &mut TestAppContext) {
        let thread_id = ThreadId::parse("first-send-switch-task").expect("thread");
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([Ok(())]);
        cx.update(|_, app| {
            view.update(app, |application, cx| {
                install_admitted_first_send(
                    application,
                    cx,
                    thread_id.clone(),
                    "keep my draft",
                    sink,
                );
                application.selected_thread =
                    Some(ThreadId::parse("other-task").expect("other thread"));
                assert!(application.suppress_stale_pending_first_send(cx));
                assert!(application.pending_first_send.is_none());
                assert!(!application.composer.read(cx).is_submitting());
                assert_eq!(application.composer.read(cx).draft(), "keep my draft");
            });
        });
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
    fn home_project_choice_updates_app_selection_and_scope(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Ok(())]);
        let beta = ProjectId::parse("home-beta").expect("fixture project");

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.test_command_sink = Some(sink);
                let options = vec![
                    ProjectOption {
                        id: ProjectId::parse("home-alpha").expect("fixture project"),
                        name: "alpha".to_owned().into(),
                    },
                    ProjectOption {
                        id: beta.clone(),
                        name: "beta".to_owned().into(),
                    },
                ];
                application.project_options.clone_from(&options);
                application.install_home_picker(options, None, application_cx);
                let home = application
                    .home_picker
                    .clone()
                    .expect("home picker installed");
                // Drive the window-free controller seams exactly as the
                // pointer/keyboard wrappers do, then route the drained action.
                home.update(application_cx, |picker, picker_cx| {
                    picker.toggle_menu(picker_cx);
                    assert!(picker.state().is_open());
                    picker.commit_row(PickerRow::Project(1), picker_cx);
                });
                application.route_home_picker_action(&home, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, _| {
                assert_eq!(application.selected_project, Some(beta.clone()));
                assert!(
                    commands.borrow().iter().any(
                        |command| matches!(command, NativeTransportCommand::SelectProject(id) if id == &beta)
                    ),
                    "choosing a home row submits the real selection command"
                );
            });
        });
    }

    #[gpui::test]
    fn thread_open_focuses_composer_for_immediate_typing(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let project = ProjectId::parse("thread-focus-project").expect("fixture project");
        let thread = ThreadId::parse("thread-focus-thread").expect("fixture thread");
        let host = cx.update(|_, app| {
            ConversationHost::mount(thread.clone(), ThemeMode::Dark, app).expect("host")
        });

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.project_options = vec![ProjectOption {
                    id: project.clone(),
                    name: "focus".to_owned().into(),
                }];
                application.selected_project = Some(project.clone());
                application.conversation_host = Some(host.clone());
                application.navigate(
                    NativeRoute::Thread {
                        project: project.clone(),
                        thread: thread.clone(),
                    },
                    application_cx,
                );
            });
        });
        cx.run_until_parked();

        // No manual focus step: opening the thread must focus the composer
        // itself, because typed text only reaches the surface holding
        // window focus. This is the reported production path with zero
        // admission bypasses: no set_disabled call anywhere.
        cx.simulate_input("hello");
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "hello"
                );
            });
        });
    }

    #[gpui::test]
    fn thread_composer_click_focuses_for_typing_after_other_control(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let project = ProjectId::parse("thread-click-project").expect("fixture project");
        let thread = ThreadId::parse("thread-click-thread").expect("fixture thread");
        let host = cx.update(|_, app| {
            ConversationHost::mount(thread.clone(), ThemeMode::Dark, app).expect("host")
        });

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.project_options = vec![ProjectOption {
                    id: project.clone(),
                    name: "click".to_owned().into(),
                }];
                application.selected_project = Some(project.clone());
                application.conversation_host = Some(host.clone());
                application.navigate(
                    NativeRoute::Thread {
                        project: project.clone(),
                        thread: thread.clone(),
                    },
                    application_cx,
                );
            });
        });
        cx.run_until_parked();
        // Deliberately park focus on another real control first, so this
        // exercises the pointer path rather than any prior focus state. No
        // direct focus call or handler invocation on the composer itself.
        cx.update(|window, app| {
            view.update(app, |application, cx| {
                window.focus(&application.profile_focus, cx)
            });
        });
        cx.run_until_parked();
        let editor = cx
            .debug_bounds(crate::native_composer::NATIVE_COMPOSER_EDITOR_SELECTOR)
            .expect("composer editor paints");
        cx.simulate_click(editor.center(), gpui::Modifiers::default());
        cx.run_until_parked();

        cx.simulate_input("hello");
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "hello"
                );
            });
        });
    }

    #[gpui::test]
    fn new_thread_composer_accepts_typed_draft_while_send_unready(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.run_until_parked();
        // Focus exactly as a pointer press does; the composer starts
        // enabled and no test bypass touches admission.
        cx.update(|window, app| {
            view.update(app, |application, cx| {
                let focus = application.composer.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            });
        });
        cx.run_until_parked();

        cx.simulate_input("hello");
        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                assert_eq!(
                    application.composer.read(application_cx).draft(),
                    "hello"
                );
                assert!(
                    !application.message_submission_is_admissible(application_cx),
                    "typing a local draft must not imply send readiness"
                );
            });
        });
    }

    #[gpui::test]
    fn home_project_intake_row_submits_real_intake_command(cx: &mut TestAppContext) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        let (sink, commands) = command_sink([Ok(())]);

        cx.update(|_, app| {
            view.update(app, |application, application_cx| {
                application.test_command_sink = Some(sink);
                let options = vec![ProjectOption {
                    id: ProjectId::parse("home-alpha").expect("fixture project"),
                    name: "alpha".to_owned().into(),
                }];
                application.project_options.clone_from(&options);
                application.install_home_picker(options, None, application_cx);
                let home = application
                    .home_picker
                    .clone()
                    .expect("home picker installed");
                home.update(application_cx, |picker, picker_cx| {
                    picker.toggle_menu(picker_cx);
                    picker.commit_row(PickerRow::NewProject, picker_cx);
                });
                application.route_home_picker_action(&home, application_cx);
            });
        });
        cx.run_until_parked();

        cx.update(|_, app| {
            view.update(app, |application, _| {
                assert!(
                    commands
                        .borrow()
                        .iter()
                        .any(|command| matches!(
                            command,
                            NativeTransportCommand::BeginProjectIntake
                        )),
                    "the home New project row starts the genuine intake flow"
                );
                assert_eq!(
                    application.intake_stage,
                    Some(NativeProjectIntakeStage::PickingDirectory)
                );
            });
        });
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
    fn sidebar_task_links_share_sliding_hover_surface(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.run_until_parked();
        let new_thread = cx
            .debug_bounds("artisan-workspace-navigation")
            .expect("New thread navigation");
        let marketplace = cx
            .debug_bounds("artisan-marketplace-navigation")
            .expect("Marketplace navigation");
        assert!(cx.debug_bounds("artisan-workspace-tabs-list").is_none());

        cx.simulate_mouse_move(new_thread.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            let hover = application.sidebar_hover.borrow();
            assert_eq!(hover.active_id(), Some("new-thread"));
        });

        cx.simulate_mouse_move(marketplace.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            let hover = application.sidebar_hover.borrow();
            assert_eq!(hover.active_id(), Some("marketplace"));
        });
    }

    #[gpui::test]
    fn sidebar_footer_shares_sliding_hover_and_spacer_clears(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds("artisan-desktop-profile-trigger")
            .expect("profile footer trigger");
        let spacer = cx
            .debug_bounds("artisan-sidebar-spacer")
            .expect("sidebar spacer");
        let marketplace = cx
            .debug_bounds("artisan-marketplace-navigation")
            .expect("Marketplace navigation");

        // The footer trigger joins the shared pill with its own target.
        cx.simulate_mouse_move(trigger.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            let hover = application.sidebar_hover.borrow();
            assert_eq!(hover.active_id(), Some("profile"));
            assert!(hover.visible());
        });

        // Entering the blank spacer hides the pill instead of stranding it,
        // retaining geometry so the next row keeps sliding.
        cx.simulate_mouse_move(spacer.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            let hover = application.sidebar_hover.borrow();
            assert_eq!(hover.active_id(), None);
            assert!(!hover.visible());
        });

        // Reentering a nav row retargets the same shared pill, sliding from
        // the retained rect instead of placing instantly.
        cx.simulate_mouse_move(marketplace.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            let hover = application.sidebar_hover.borrow();
            assert_eq!(hover.active_id(), Some("marketplace"));
            assert!(hover.visible());
            assert!(
                hover.transition().is_some(),
                "row-to-row must slide, not jump"
            );
        });

        // The footer seal spans the sidebar edges: exactly 10px past the
        // trigger on each side, matching the sidebar padding it bleeds.
        let divider = cx
            .debug_bounds("artisan-sidebar-footer-divider")
            .expect("footer divider");
        assert_eq!(f32::from(divider.left()), f32::from(trigger.left()) - 10.0);
        assert_eq!(
            f32::from(divider.right()),
            f32::from(trigger.right()) + 10.0
        );
    }

    #[gpui::test]
    fn profile_actions_share_sliding_hover_and_keyboard_syncs_pill(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));

        let settings = cx
            .debug_bounds("artisan-desktop-profile-action-0")
            .expect("profile Settings action");
        let usage = cx
            .debug_bounds("artisan-desktop-profile-action-1")
            .expect("profile Usage action");

        cx.simulate_mouse_move(settings.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert_eq!(application.profile_menu.highlighted_index(), Some(0));
            assert_eq!(
                application.profile_hover.borrow().active_id(),
                Some("profile-settings")
            );
            assert!(application.profile_hover.borrow().visible());
        });

        cx.simulate_mouse_move(usage.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert_eq!(application.profile_menu.highlighted_index(), Some(1));
            assert_eq!(
                application.profile_hover.borrow().active_id(),
                Some("profile-usage")
            );
        });

        // Leaving the action surface clears a pointer-owned pill while the
        // open menu keeps its keyboard highlight.
        let trigger = cx
            .debug_bounds("artisan-desktop-profile-trigger")
            .expect("profile trigger");
        cx.simulate_mouse_move(trigger.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(application.profile_menu.is_open());
            assert_eq!(application.profile_menu.highlighted_index(), Some(1));
            assert_eq!(application.profile_hover.borrow().active_id(), None);
            assert!(!application.profile_hover.borrow().visible());
        });

        // Re-entering restores the pointer-owned pill under the cursor.
        cx.simulate_mouse_move(usage.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert_eq!(application.profile_menu.highlighted_index(), Some(1));
            assert_eq!(
                application.profile_hover.borrow().active_id(),
                Some("profile-usage")
            );
            assert!(application.profile_hover.borrow().visible());
        });

        cx.simulate_keystrokes("home");
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert_eq!(application.profile_menu.highlighted_index(), Some(0));
            assert_eq!(
                application.profile_hover.borrow().active_id(),
                Some("profile-settings")
            );
            assert!(application.profile_hover.borrow().visible());
        });

        // A keyboard-owned pill survives a real surface leave: the pointer
        // resting elsewhere must not discard keyboard navigation.
        let usage_region = cx
            .debug_bounds(crate::native_profile_usage::PROFILE_USAGE_SELECTOR)
            .expect("profile usage region");
        cx.simulate_mouse_move(usage_region.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(application.profile_menu.is_open());
            assert_eq!(application.profile_menu.highlighted_index(), Some(0));
            assert_eq!(
                application.profile_hover.borrow().active_id(),
                Some("profile-settings")
            );
            assert!(application.profile_hover.borrow().visible());
        });

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(!application.profile_menu.is_open());
            assert_eq!(application.profile_hover.borrow().active_id(), None);
            assert!(!application.profile_hover.borrow().visible());
        });
    }

    #[gpui::test]
    fn profile_usage_small_content_keeps_natural_height(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let scroller = cx
            .debug_bounds("artisan-profile-usage-scroll")
            .expect("usage scroller");
        let height = f32::from(scroller.size.height);
        assert!(
            height > 0.0 && height < 120.0,
            "short usage content must keep its natural height, got {height}"
        );
        cx.update(|_, app| {
            assert_eq!(
                f32::from(view.read(app).profile_usage_scroll.max_offset().y),
                0.0
            );
        });
        assert!(cx.debug_bounds("artisan-desktop-profile-header").is_some());
        assert!(
            cx.debug_bounds("artisan-desktop-profile-actions-hover-surface")
                .is_some()
        );
    }

    #[gpui::test]
    fn profile_usage_tall_content_caps_with_viewport(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                install_connected_profile_usage(application, sink);
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
        cx.run_until_parked();
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let tall = cx
            .debug_bounds("artisan-profile-usage-scroll")
            .expect("usage scroller");
        assert!(
            f32::from(tall.size.height) > 280.0,
            "tall usage content must expand past the old 280px cap"
        );
        cx.update(|_, app| {
            assert!(
                f32::from(view.read(app).profile_usage_scroll.max_offset().y) > 0.0,
                "the tall usage fixture must scroll"
            );
        });
        assert!(cx.debug_bounds("artisan-desktop-profile-header").is_some());
        assert!(
            cx.debug_bounds("artisan-desktop-profile-actions-hover-surface")
                .is_some()
        );
        // The inline refresh control is present once an engine answered.
        assert!(
            cx.debug_bounds("artisan-profile-usage-refresh-profile-test-alpha")
                .is_some()
        );
        // The zero-percent window is real data: its meter row is rendered.
        let zero_meter = cx
            .debug_bounds("artisan-profile-usage-meter-profile-test-alpha-session")
            .expect("zero-percent meter row");
        // The meter bar keeps the exact source 72px width.
        let zero_bar = cx
            .debug_bounds("artisan-profile-usage-meter-profile-test-alpha-session-bar")
            .expect("zero-percent meter bar");
        assert_eq!(f32::from(zero_bar.size.width), 72.0);
        // No tooltip until a meter is hovered.
        assert!(cx.debug_bounds("artisan-profile-usage-tooltip").is_none());
        cx.simulate_mouse_move(zero_meter.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(cx.debug_bounds("artisan-profile-usage-tooltip").is_some());
        // Leaving the meter row dismisses its tooltip.
        let header_top = cx
            .debug_bounds("artisan-desktop-profile-header")
            .expect("profile header");
        cx.simulate_mouse_move(header_top.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(cx.debug_bounds("artisan-profile-usage-tooltip").is_none());

        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(320.0)));
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let capped = cx
            .debug_bounds("artisan-profile-usage-scroll")
            .expect("usage scroller");
        assert!(
            f32::from(capped.size.height) < 120.0,
            "a short window must shrink the usage area instead of overflowing"
        );
        let header = cx
            .debug_bounds("artisan-desktop-profile-header")
            .expect("profile header");
        assert!(
            header.top() >= gpui::px(0.0),
            "the header must stay inside a short window"
        );
        assert!(
            cx.debug_bounds("artisan-desktop-profile-actions-hover-surface")
                .is_some()
        );

        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(200.0)));
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let collapsed = cx
            .debug_bounds("artisan-profile-usage-scroll")
            .expect("usage scroller");
        assert!(
            f32::from(collapsed.size.height) <= 1.0,
            "a tiny window must clamp the usage area instead of going negative"
        );
    }

    #[gpui::test]
    fn profile_usage_wheel_scrolls_once_and_dismiss_cancels(cx: &mut TestAppContext) {
        cx.update(|app| app.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                install_connected_profile_usage(application, sink);
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
        cx.run_until_parked();
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let maximum = cx.update(|_, app| {
            let maximum = f32::from(view.read(app).profile_usage_scroll.max_offset().y);
            assert!(maximum > 0.0, "the tall usage fixture must scroll");
            maximum
        });
        let scroll_center = cx
            .debug_bounds("artisan-profile-usage-scroll")
            .expect("usage scroller")
            .center();

        // A lines wheel queues a bounded target without jumping there.
        cx.update(|_, app| app.set_reduce_motion(false));
        let line = cx.update(|window, _| f32::from(window.line_height()));
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: scroll_center,
            delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -3.0)),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.update(|_, app| {
            let application = view.read(app);
            let target = application.profile_usage_scroll_state.target();
            assert_eq!(target, (-3.0 * line).clamp(-maximum, 0.0));
            let offset = f32::from(application.profile_usage_scroll.offset().y);
            assert!(
                offset >= target && offset <= 0.0,
                "the wheel must not jump straight to its target"
            );
        });

        // A precise pixel wheel applies exactly once and cancels inertia.
        let expected_pixel = cx.update(|_, app| {
            let application = view.read(app);
            (f32::from(application.profile_usage_scroll.offset().y) - 7.0).clamp(
                -f32::from(application.profile_usage_scroll.max_offset().y),
                0.0,
            )
        });
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: scroll_center,
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.0), gpui::px(-7.0))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.update(|_, app| {
            let application = view.read(app);
            assert_eq!(
                f32::from(application.profile_usage_scroll.offset().y),
                expected_pixel
            );
            assert!(!application.profile_usage_scroll_state.active());
        });

        // Reduced motion settles a lines wheel directly.
        cx.update(|_, app| app.set_reduce_motion(true));
        let expected_reduced = cx.update(|_, app| {
            let application = view.read(app);
            (f32::from(application.profile_usage_scroll.offset().y) - line).clamp(
                -f32::from(application.profile_usage_scroll.max_offset().y),
                0.0,
            )
        });
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: scroll_center,
            delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -1.0)),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.update(|_, app| {
            let application = view.read(app);
            assert_eq!(
                f32::from(application.profile_usage_scroll.offset().y),
                expected_reduced
            );
            assert!(!application.profile_usage_scroll_state.active());
        });

        // Dismissing with a queued target cancels the pending motion.
        cx.update(|_, app| app.set_reduce_motion(false));
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: scroll_center,
            delta: gpui::ScrollDelta::Lines(gpui::point(0.0, -3.0)),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(!application.profile_menu.is_open());
            assert_eq!(
                application.profile_usage_scroll_state.target(),
                f32::from(application.profile_usage_scroll.offset().y)
            );
            assert!(!application.profile_usage_scroll_state.active());
        });
    }

    #[gpui::test]
    fn profile_usage_hides_providers_without_data(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                application.test_command_sink = Some(sink);
                application.profile_usage.entries.push(reported_usage_entry(
                    "profile-test-hidden",
                    "Hidden",
                    Vec::new(),
                ));
                application.profile_usage.entries.push(NativeUsageEntry {
                    engine_id: "profile-test-unauth".to_owned(),
                    display_name: "Unauth".to_owned(),
                    report: Some(NativeUsageReport {
                        engine_id: "profile-test-unauth".to_owned(),
                        display_name: "Unauth".to_owned(),
                        authentication: NativeUsageAuthentication::Unauthenticated,
                        account_email: None,
                        quota_surface: NativeUsageQuotaSurface::Supported,
                        windows: vec![reported_usage_window(
                            "session",
                            NativeUsageCadence::Session,
                            None,
                            40.0,
                        )],
                        failure: None,
                    }),
                    failure: None,
                    fetched_at_ms: Some(1_000_000),
                });
            });
        });
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        // Neither the empty nor the unauthenticated provider paints a meter.
        assert!(
            cx.debug_bounds("artisan-profile-usage-meter-profile-test-hidden-session")
                .is_none()
        );
        assert!(
            cx.debug_bounds("artisan-profile-usage-meter-profile-test-unauth-session")
                .is_none()
        );
        assert!(
            cx.debug_bounds("artisan-profile-usage-refresh-profile-test-hidden")
                .is_none()
        );
        // The menu stays mounted with header and actions around the source
        // empty state instead of fabricated rows.
        assert!(cx.debug_bounds("artisan-desktop-profile-header").is_some());
        assert!(
            cx.debug_bounds("artisan-desktop-profile-actions-hover-surface")
                .is_some()
        );
    }

    #[gpui::test]
    fn profile_refresh_swap_interrupts_from_current_values(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                application.test_command_sink = Some(sink);
                application.profile_usage.entries.push(reported_usage_entry(
                    "swap-test",
                    "Swap",
                    vec![reported_usage_window(
                        "session",
                        NativeUsageCadence::Session,
                        None,
                        40.0,
                    )],
                ));
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
        cx.run_until_parked();
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let control = cx
            .debug_bounds("artisan-profile-usage-refresh-swap-test")
            .expect("refresh control");

        // Hovering arms the action target from the resting reading values.
        cx.simulate_mouse_move(control.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let swaps = view.read(app).profile_refresh_swap.borrow();
            let swap = swaps.get("swap-test").expect("swap state");
            assert_eq!(swap.to, [0.0, 1.0, 0.0]);
            assert_eq!(swap.from, [1.0, 0.0, 0.0]);
        });

        // A mid-flight step moves values without jumping to either end.
        let now = super::profile_usage_now_ms();
        cx.update(|_, app| {
            let application = view.read(app);
            application
                .profile_refresh_swap
                .borrow_mut()
                .get_mut("swap-test")
                .expect("swap state")
                .started_ms = now - 75;
            assert!(application.step_profile_swaps(now));
        });
        cx.update(|_, app| {
            let binding = view.read(app).profile_refresh_swap.borrow();
            let swap = binding.get("swap-test").expect("swap state");
            assert!(swap.displayed[0] > 0.0 && swap.displayed[0] < 1.0);
            assert!(swap.displayed[1] > 0.0 && swap.displayed[1] < 1.0);
            // Offsets travel with opacity: the leaving reading heads
            // upward from zero while the entering action arrives from
            // below, neither jumping to an endpoint.
            assert!(swap.off_displayed[0] > -4.0 && swap.off_displayed[0] < 0.0);
            assert!(swap.off_displayed[1] > 0.0 && swap.off_displayed[1] < 4.0);
        });

        // Leaving retargets from the mid-flight values, not from rest.
        let header = cx
            .debug_bounds("artisan-desktop-profile-header")
            .expect("profile header");
        cx.simulate_mouse_move(header.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let binding = view.read(app).profile_refresh_swap.borrow();
            let swap = binding.get("swap-test").expect("swap state");
            assert_eq!(swap.to, [1.0, 0.0, 0.0]);
            assert!(swap.from[0] > 0.0 && swap.from[0] < 1.0);
            assert!(swap.from[1] > 0.0 && swap.from[1] < 1.0);
            // The retained offsets continue without sign flips: the
            // reading keeps leaving upward, the action returns downward.
            assert!(swap.off_from[0] > -4.0 && swap.off_from[0] < 0.0);
            assert!(swap.off_from[1] > 0.0 && swap.off_from[1] < 4.0);
            assert_eq!(swap.off_to, [0.0, 4.0, 4.0]);
        });

        // A far-future step settles exactly on the reading values.
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(!application.step_profile_swaps(now + 100_000));
            let binding = application.profile_refresh_swap.borrow();
            let swap = binding.get("swap-test").expect("swap state");
            assert_eq!(swap.displayed, [1.0, 0.0, 0.0]);
            assert_eq!(swap.off_displayed, [0.0, 4.0, 4.0]);
        });

        // A reduced-motion retarget with a changed target settles every
        // endpoint at once, and a queued step afterwards cannot regress.
        cx.update(|_, app| {
            let application = view.read(app);
            application.retarget_profile_swap("swap-test", super::RefreshSwapTarget::Loading, true);
            let now = super::profile_usage_now_ms();
            application
                .profile_refresh_swap
                .borrow_mut()
                .get_mut("swap-test")
                .expect("swap state")
                .started_ms = now;
            assert!(!application.step_profile_swaps(now));
            let binding = application.profile_refresh_swap.borrow();
            let swap = binding.get("swap-test").expect("swap state");
            assert_eq!(swap.displayed, [0.0, 0.0, 1.0]);
            assert_eq!(swap.to, [0.0, 0.0, 1.0]);
            assert_eq!(swap.off_displayed, [-4.0, 4.0, 0.0]);
        });

        // A reduced-motion change settles mid-flight even though the target
        // itself is unchanged.
        cx.simulate_mouse_move(control.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        let mid = super::profile_usage_now_ms();
        cx.update(|_, app| {
            let application = view.read(app);
            application
                .profile_refresh_swap
                .borrow_mut()
                .get_mut("swap-test")
                .expect("swap state")
                .started_ms = mid - 75;
            assert!(application.step_profile_swaps(mid));
            application.retarget_profile_swap("swap-test", super::RefreshSwapTarget::Action, true);
            let binding = application.profile_refresh_swap.borrow();
            let swap = binding.get("swap-test").expect("swap state");
            assert_eq!(swap.displayed, [0.0, 1.0, 0.0]);
            assert_eq!(swap.off_displayed, [-4.0, 0.0, 4.0]);
        });

        // Reduced motion settles a retarget instantly.
        cx.update(|_, app| app.set_reduce_motion(true));
        cx.simulate_mouse_move(control.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let binding = view.read(app).profile_refresh_swap.borrow();
            let swap = binding.get("swap-test").expect("swap state");
            assert_eq!(swap.to, [0.0, 1.0, 0.0]);
            assert_eq!(swap.displayed, [0.0, 1.0, 0.0]);
        });
    }

    #[gpui::test]
    fn profile_engine_blocks_share_consistent_spacing(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                application.test_command_sink = Some(sink);
                // Three byte-identical providers in first, middle, and last
                // position: only consistent gap/padding keeps every block
                // the same height with the same rhythm between them.
                for engine_id in ["spacing-a", "spacing-b", "spacing-c"] {
                    application.profile_usage.entries.push(reported_usage_entry(
                        engine_id,
                        "Same",
                        vec![
                            reported_usage_window(
                                "session",
                                NativeUsageCadence::Session,
                                None,
                                30.0,
                            ),
                            reported_usage_window(
                                "weekly-model",
                                NativeUsageCadence::Weekly,
                                Some("Model"),
                                50.0,
                            ),
                        ],
                    ));
                }
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
        cx.run_until_parked();
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let scroller = cx
            .debug_bounds("artisan-profile-usage-scroll")
            .expect("usage scroller");
        let first = cx
            .debug_bounds("artisan-profile-usage-engine-spacing-a")
            .expect("first engine block");
        let middle = cx
            .debug_bounds("artisan-profile-usage-engine-spacing-b")
            .expect("middle engine block");
        let last = cx
            .debug_bounds("artisan-profile-usage-engine-spacing-c")
            .expect("last engine block");
        assert_eq!(f32::from(first.size.height), f32::from(middle.size.height));
        assert_eq!(f32::from(middle.size.height), f32::from(last.size.height));
        // Engine separators keep one 1px rule with 4px margins on each side.
        assert_eq!(f32::from(middle.top() - first.bottom()), 9.0);
        assert_eq!(f32::from(last.top() - middle.bottom()), 9.0);
        // The section contributes the single outer 4px inset on both ends.
        assert_eq!(f32::from(first.top() - scroller.top()), 4.0);
        assert_eq!(f32::from(scroller.bottom() - last.bottom()), 4.0);
    }

    #[gpui::test]
    fn profile_tip_tween_runs_up_once_then_carries_across_rows(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                install_connected_profile_usage(application, sink);
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
        cx.run_until_parked();
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));

        // Alpha's session window is unused: 100% remains, so the first
        // reading runs up from just short of it.
        let alpha = cx
            .debug_bounds("artisan-profile-usage-meter-profile-test-alpha-session")
            .expect("alpha meter row");
        cx.simulate_mouse_move(alpha.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let tween = *view.read(app).profile_tip_tween.borrow();
            assert_eq!(tween.to, 100.0);
            assert_eq!(tween.from, 92.0);
            assert!(tween.seen);
        });

        // Beta's session window is 21% used: the displayed value carries
        // across while only the target moves.
        let beta = cx
            .debug_bounds("artisan-profile-usage-meter-profile-test-beta-session")
            .expect("beta meter row");
        cx.simulate_mouse_move(beta.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let tween = *view.read(app).profile_tip_tween.borrow();
            assert_eq!(tween.to, 79.0);
            assert_eq!(tween.from, 92.0);
            let displayed = tween.displayed;
            assert!(
                displayed >= 79.0 && displayed <= 92.0,
                "the carried value must ease toward its target, never restart"
            );
        });

        // Reduced motion settles the shared value directly.
        cx.update(|_, app| app.set_reduce_motion(true));
        cx.simulate_mouse_move(alpha.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let tween = *view.read(app).profile_tip_tween.borrow();
            assert_eq!(tween.to, 100.0);
            assert_eq!(tween.displayed, 100.0);
        });
    }

    #[gpui::test]
    fn profile_refresh_spinner_keeps_control_width(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                install_connected_profile_usage(application, sink);
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
        cx.run_until_parked();
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        let idle = cx
            .debug_bounds("artisan-profile-usage-refresh-profile-test-alpha")
            .expect("refresh control");
        let idle_width = f32::from(idle.size.width);
        assert!(
            cx.debug_bounds("artisan-profile-usage-refresh-profile-test-alpha-spinner")
                .is_some()
        );

        cx.update(|_, app| {
            view.update(app, |application, cx| {
                application
                    .profile_usage
                    .refreshing_engine_ids
                    .push("profile-test-alpha".to_owned());
                cx.notify();
            });
        });
        cx.run_until_parked();
        let loading = cx
            .debug_bounds("artisan-profile-usage-refresh-profile-test-alpha")
            .expect("refresh control");
        assert_eq!(f32::from(loading.size.width), idle_width);
        assert!(
            cx.debug_bounds("artisan-profile-usage-refresh-profile-test-alpha-spinner")
                .is_some()
        );
    }

    #[gpui::test]
    fn profile_refresh_focus_enter_refreshes_once(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, commands) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                application.test_command_sink = Some(sink);
                application.profile_usage.entries.push(reported_usage_entry(
                    "codex",
                    "Codex",
                    vec![reported_usage_window(
                        "session",
                        NativeUsageCadence::Session,
                        None,
                        50.0,
                    )],
                ));
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(1200.0), gpui::px(900.0)));
        cx.run_until_parked();
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        // The fresh Codex reading needs no load on open; only the focused
        // Enter may dispatch its refresh.
        let focus = cx.update(|_, app| {
            view.read(app)
                .profile_refresh_focus
                .borrow()
                .iter()
                .find(|(id, _)| id == "codex")
                .map(|(_, handle)| handle.clone())
                .expect("refresh focus handle")
        });
        let codex_reads = || {
            commands
                .borrow()
                .iter()
                .filter(|command| {
                    matches!(
                        command,
                        NativeTransportCommand::ReadAccountUsage { engine_id, .. }
                        if engine_id == "codex"
                    )
                })
                .count()
        };
        assert_eq!(codex_reads(), 0);
        cx.update(|window, app| window.focus(&focus, app));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert_eq!(codex_reads(), 1);
    }

    #[gpui::test]
    fn profile_menu_plays_shared_popup_motion(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.update(|_, app| app.set_reduce_motion(true));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(application.profile_menu.is_open());
            assert_eq!(
                application.profile_menu_motion.borrow().phase(),
                crate::native_model_selector::PickerMenuPhase::Open
            );
        });
        assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(!application.profile_menu.is_open());
            assert_eq!(
                application.profile_menu_motion.borrow().phase(),
                crate::native_model_selector::PickerMenuPhase::Hidden
            );
        });
        assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_none());

        cx.update(|_, app| app.set_reduce_motion(false));
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(application.profile_menu.is_open());
            assert_eq!(
                application.profile_menu_motion.borrow().phase(),
                crate::native_model_selector::PickerMenuPhase::Opening
            );
        });
        assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_some());
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(!application.profile_menu.is_open());
            assert_eq!(
                application.profile_menu_motion.borrow().phase(),
                crate::native_model_selector::PickerMenuPhase::Closing
            );
        });
        // The retained exit presentation stays mounted through Closing.
        assert!(cx.debug_bounds("artisan-desktop-profile-menu").is_some());
    }

    #[gpui::test]
    fn profile_tip_clamps_into_a_narrow_viewport(cx: &mut TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        let (sink, _) = command_sink([]);
        cx.update(|_, app| {
            view.update(app, |application, _| {
                install_connected_profile_usage(application, sink);
            });
        });
        cx.simulate_resize(gpui::size(gpui::px(400.0), gpui::px(900.0)));
        cx.run_until_parked();
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        let alpha = cx
            .debug_bounds("artisan-profile-usage-meter-profile-test-alpha-session")
            .expect("alpha meter row");
        cx.simulate_mouse_move(alpha.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        let tooltip = cx
            .debug_bounds("artisan-profile-usage-tooltip")
            .expect("usage tooltip");
        assert!(f32::from(tooltip.left()) <= 169.0);
        assert!(f32::from(tooltip.right()) <= 393.0);
    }

    #[test]
    fn profile_name_capitalizes_first_letter_only() {
        assert_eq!(super::capitalize_label("sander"), "Sander");
        assert_eq!(
            super::capitalize_label("DESKTOP-96USC6J"),
            "Desktop-96usc6j"
        );
        assert_eq!(super::capitalize_label(""), "");
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
    fn profile_menu_usage_action_keeps_menu_open_and_never_invents_readings(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|window, cx| NativeApplication::new(None, window, cx));
        cx.update(|window, app| {
            view.update(app, |view, cx| window.focus(&view.profile_focus, cx));
        });
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
        cx.update(|_, app| {
            let application = view.read(app);
            assert!(application.profile_menu.is_open());
            assert!(application.profile_usage.entries.is_empty());
            let item_ids = application
                .profile_menu
                .entries()
                .iter()
                .filter_map(|entry| entry.as_item())
                .map(|item| item.id.as_ref())
                .collect::<Vec<_>>();
            assert_eq!(item_ids, vec!["settings", "usage"]);
        });
        assert!(
            cx.debug_bounds(crate::native_profile_usage::PROFILE_USAGE_SELECTOR)
                .is_some()
        );
        cx.simulate_keystrokes("end enter");
        cx.run_until_parked();
        assert!(cx.update(|_, app| view.read(app).profile_menu.is_open()));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!cx.update(|_, app| view.read(app).profile_menu.is_open()));
    }

    #[gpui::test]
    fn command_shortcut_opens_palette_without_persistent_titlebar_search(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) =
            cx.add_window_view(|window, view_cx| NativeApplication::new(None, window, view_cx));
        cx.update(|_, app| super::bind_native_actions(app));
        cx.run_until_parked();
        // The titlebar no longer paints a persistent search input; the
        // palette lives behind the keyboard shortcut.
        cx.update(|_, app| {
            assert!(!view.read(app).command_menu.read(app).state().is_open());
        });
        assert!(cx.debug_bounds(COMMAND_MENU_INPUT_SELECTOR).is_none());

        // Ctrl+K opens the working palette: focused input, result list, and
        // dialog scrim — with no titlebar dropdown.
        cx.simulate_keystrokes("ctrl-k");
        cx.run_until_parked();
        cx.update(|window, app| {
            let application = view.read(app);
            let menu = application.command_menu.read(app);
            assert!(menu.state().is_open());
            assert!(menu.input_focus().is_focused(window));
        });
        assert!(cx.debug_bounds(COMMAND_MENU_INPUT_SELECTOR).is_some());
        assert!(cx.debug_bounds(COMMAND_MENU_LIST_SELECTOR).is_some());
        assert!(cx.debug_bounds(COMMAND_MENU_DROPDOWN_SELECTOR).is_none());
        assert!(cx
            .debug_bounds("artisan-native-command-menu-scrim")
            .is_some());

        // Escape closes the palette and restores root focus, removing the
        // transient input with it.
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        cx.update(|window, app| {
            let application = view.read(app);
            assert!(!application.command_menu.read(app).state().is_open());
            assert!(application.focus_handle.is_focused(window));
        });
        assert!(cx.debug_bounds(COMMAND_MENU_INPUT_SELECTOR).is_none());
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
                install_configured_engine_settings(application, application_cx);
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
                install_configured_engine_settings(application, application_cx);
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
