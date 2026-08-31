//! Shipping GPUI application assembly for the native Artisan workflow.
//!
//! This module owns only application-thread composition. Installation,
//! transport, process custody, and protocol work live in
//! `native_transport_service`; the conversation host remains the sole owner of
//! controller and surface policy.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{
    cell::Cell,
    process::ExitCode,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use artisan_domain::{ConversationSnapshot, ProjectId, ProjectListing, ThreadId};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    App, AppContext as _, Application, Bounds, Context, Div, Entity, FocusHandle, KeyBinding,
    Render, Subscription, Task, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    prelude::{InteractiveElement as _, IntoElement, ParentElement as _, Styled as _},
    px, size,
};

use crate::native_transport_service::{
    CommandSendError, EventReceiveError, NativeProjectIntakeOperation, NativeProjectIntakeStage,
    NativeTransportCommand, NativeTransportEvent, NativeTransportService, ServiceFailure,
    ServiceFailureCategory, ServiceFailureStage, ServiceStopStatus,
};
use crate::{
    conversation_delivery_machine::{ConversationDeliveryEffect, ConversationDeliveryEvent},
    conversation_host::{CONVERSATION_HOST_MAX_EFFECTS, ConversationHost, ConversationHostEffect},
    conversation_state_machine::{ConversationStateEffect, ConversationStateEvent},
    project_picker::{ProjectOption, ProjectPickerAction, ProjectPickerView},
    shell::{ShellFrameStyle, shell_rail},
};

actions!(native_application, [Quit, NextTabStop, PreviousTabStop]);

/// The one shipping application title.
pub(crate) const WINDOW_TITLE: &str = "Artisan";

/// Stable selector for the real application root.
pub(crate) const NATIVE_ROOT_SELECTOR: &str = "artisan-native-application";

/// Stable selector for the state panel.
pub(crate) const NATIVE_STATUS_SELECTOR: &str = "artisan-native-status";

const NATIVE_KEY_CONTEXT: &str = "artisan-native-application";
const SURFACE_WIDTH: f32 = 1_024.0;
const SURFACE_HEIGHT: f32 = 720.0;
const POLL_INTERVAL: Duration = Duration::from_millis(16);

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

/// The real native window root and its application-thread entities.
pub struct NativeApplication {
    theme: ArtisanTheme,
    focus_handle: FocusHandle,
    service: Option<Arc<NativeTransportService>>,
    picker: Option<Entity<ProjectPickerView>>,
    picker_subscription: Option<Subscription>,
    project_options: Vec<ProjectOption>,
    selected_project: Option<ProjectId>,
    selected_thread: Option<ThreadId>,
    pending_thread: Option<ThreadId>,
    pending_snapshot: Option<ConversationSnapshot>,
    conversation_host: Option<Entity<ConversationHost>>,
    conversation_host_subscription: Option<Subscription>,
    conversation_effects: Vec<ConversationHostEffect>,
    last_picker_action: Option<ProjectPickerAction>,
    state: NativeViewState,
    intake_stage: Option<NativeProjectIntakeStage>,
    intake_failure_operation: Option<NativeProjectIntakeOperation>,
    intake_retry_available: bool,
    intake_restore_state: Option<NativeViewState>,
    service_stopped: bool,
    poll_task: Option<Task<()>>,
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
        focus_handle.focus(window);
        let state = if service.is_some() {
            NativeViewState::Loading
        } else {
            NativeViewState::Failure(ServiceFailure {
                stage: ServiceFailureStage::EventBridge,
                category: ServiceFailureCategory::ChannelClosed,
            })
        };
        Self {
            theme: ArtisanTheme::for_mode(ThemeMode::Dark),
            focus_handle,
            service,
            picker: None,
            picker_subscription: None,
            project_options: Vec::new(),
            selected_project: None,
            selected_thread: None,
            pending_thread: None,
            pending_snapshot: None,
            conversation_host: None,
            conversation_host_subscription: None,
            conversation_effects: Vec::with_capacity(CONVERSATION_HOST_MAX_EFFECTS),
            last_picker_action: None,
            state,
            intake_stage: None,
            intake_failure_operation: None,
            intake_retry_available: false,
            intake_restore_state: None,
            service_stopped: false,
            poll_task: None,
        }
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

    /// Returns the host entity when a real thread is selected.
    #[must_use]
    pub fn conversation_host(&self) -> Option<&Entity<ConversationHost>> {
        self.conversation_host.as_ref()
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
                    self.set_failure(
                        ServiceFailure {
                            stage: ServiceFailureStage::EventBridge,
                            category: ServiceFailureCategory::ChannelClosed,
                        },
                        cx,
                    );
                    self.service_stopped = true;
                    break;
                }
            }
        }
        for event in events {
            self.handle_service_event(event, cx);
        }
        self.try_mount_pending_thread(cx);
        !self.service_stopped
    }

    fn handle_service_event(&mut self, event: NativeTransportEvent, cx: &mut Context<Self>) {
        match event {
            NativeTransportEvent::Starting => {
                self.state = NativeViewState::Loading;
                cx.notify();
            }
            NativeTransportEvent::Projects(listing) => self.handle_projects(&listing, cx),
            NativeTransportEvent::Threads {
                project_id,
                listing,
            } => self.handle_threads(&project_id, &listing, cx),
            NativeTransportEvent::Snapshot(snapshot) => self.handle_snapshot(snapshot, cx),
            NativeTransportEvent::ProjectIntakeProgress(stage) => {
                self.handle_intake_progress(stage, cx)
            }
            NativeTransportEvent::ProjectIntakeCancelled => self.handle_intake_cancelled(cx),
            NativeTransportEvent::ProjectIntakeReady {
                projects,
                project_id,
                threads,
                thread_id,
            } => self.handle_intake_ready(projects, project_id, threads, thread_id, cx),
            NativeTransportEvent::ProjectIntakeFailed {
                operation,
                failure,
                retryable,
            } => self.handle_intake_failed(operation, failure, retryable, cx),
            NativeTransportEvent::EmptyProjects => {
                self.pending_thread = None;
                self.pending_snapshot = None;
                self.retire_host(cx);
                self.state = NativeViewState::EmptyProjects;
                cx.notify();
            }
            NativeTransportEvent::EmptyThreads { project_id } => {
                if self.selected_project.as_ref() != Some(&project_id) {
                    self.set_failure(invalid_service_failure(), cx);
                    return;
                }
                self.pending_thread = None;
                self.pending_snapshot = None;
                self.retire_host(cx);
                self.state = NativeViewState::EmptyThreads;
                cx.notify();
            }
            NativeTransportEvent::Failed(failure) => self.set_failure(failure, cx),
            NativeTransportEvent::Stopped(status) => {
                self.service_stopped = true;
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
                    cx.notify();
                }
            }
        }
    }

    fn handle_intake_progress(&mut self, stage: NativeProjectIntakeStage, cx: &mut Context<Self>) {
        if self.intake_restore_state.is_none() {
            self.intake_restore_state = Some(self.state.clone());
        }
        self.intake_stage = Some(stage);
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        self.set_picker_disabled(true, cx);
        cx.notify();
    }

    fn handle_intake_cancelled(&mut self, cx: &mut Context<Self>) {
        self.intake_stage = None;
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        if let Some(state) = self.intake_restore_state.take() {
            self.state = state;
        }
        let options = self.project_options.clone();
        self.install_picker(options, self.selected_project.clone(), cx);
        cx.notify();
    }

    fn handle_intake_failed(
        &mut self,
        operation: NativeProjectIntakeOperation,
        failure: ServiceFailure,
        retryable: bool,
        cx: &mut Context<Self>,
    ) {
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
        cx.notify();
    }

    fn handle_intake_ready(
        &mut self,
        projects: ProjectListing,
        project_id: ProjectId,
        threads: artisan_domain::ThreadListing,
        thread_id: ThreadId,
        cx: &mut Context<Self>,
    ) {
        if !ready_membership_is_valid(&projects, &project_id, &threads, &thread_id) {
            self.handle_intake_failed(
                NativeProjectIntakeOperation::RefreshThreads,
                invalid_service_failure(),
                false,
                cx,
            );
            return;
        }
        let options = project_options_from_listing(&projects);
        self.project_options = options.clone();
        self.selected_project = Some(project_id.clone());
        self.selected_thread = None;
        self.pending_thread = Some(thread_id);
        self.pending_snapshot = None;
        self.intake_stage = None;
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        self.intake_restore_state = None;
        self.state = NativeViewState::Loading;
        self.install_picker(options, Some(project_id), cx);
        self.try_mount_pending_thread(cx);
        cx.notify();
    }

    fn handle_projects(&mut self, listing: &ProjectListing, cx: &mut Context<Self>) {
        let options = project_options_from_listing(listing);
        self.retire_host(cx);
        self.project_options.clone_from(&options);
        self.selected_project = options.first().map(|project| project.id.clone());
        self.pending_thread = None;
        self.pending_snapshot = None;
        self.install_picker(options, self.selected_project.clone(), cx);
        if self.project_options.is_empty() {
            self.state = NativeViewState::EmptyProjects;
        } else {
            self.state = NativeViewState::LoadingThreads;
        }
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
    }

    fn set_picker_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        let Some(picker) = self.picker.clone() else {
            return;
        };
        picker.update(cx, |picker, picker_cx| {
            picker.set_disabled(disabled, picker_cx);
        });
    }

    fn handle_threads(
        &mut self,
        project_id: &ProjectId,
        listing: &artisan_domain::ThreadListing,
        cx: &mut Context<Self>,
    ) {
        if self.selected_project.as_ref() != Some(project_id)
            || listing
                .threads()
                .iter()
                .any(|thread| &thread.project_id != project_id)
        {
            self.set_failure(invalid_service_failure(), cx);
            return;
        }
        self.pending_snapshot = None;
        self.pending_thread = listing
            .threads()
            .first()
            .map(|thread| thread.thread_id.clone());
        if self.pending_thread.is_none() {
            self.retire_host(cx);
            self.state = NativeViewState::EmptyThreads;
        } else {
            self.state = NativeViewState::Loading;
            self.try_mount_pending_thread(cx);
        }
        cx.notify();
    }

    fn handle_snapshot(&mut self, snapshot: ConversationSnapshot, cx: &mut Context<Self>) {
        let thread_id = snapshot.thread_id().clone();
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
            self.pump_host_boundary(host, cx);
            cx.notify();
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
        match picker_route(&action, &self.project_options) {
            Ok(PickerRoute::Select(project_id)) => {
                self.intake_stage = None;
                self.intake_failure_operation = None;
                self.intake_retry_available = false;
                self.intake_restore_state = None;
                self.pending_thread = None;
                self.pending_snapshot = None;
                self.retire_host(cx);
                self.selected_project = Some(project_id.clone());
                self.state = NativeViewState::Loading;
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
                match service.submit(NativeTransportCommand::SelectProject(project_id)) {
                    Ok(()) => cx.notify(),
                    Err(error) => {
                        self.set_failure(command_failure(error), cx);
                    }
                }
            }
            Ok(PickerRoute::BeginProjectIntake) => {
                self.submit_intake_command(cx);
            }
            Err(failure) => self.set_failure(failure, cx),
        }
    }

    fn submit_intake_command(&mut self, cx: &mut Context<Self>) {
        let Some(service) = self.service.clone() else {
            self.handle_intake_failed(
                NativeProjectIntakeOperation::PickDirectory,
                ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                },
                false,
                cx,
            );
            return;
        };
        let retryable = self.intake_retry_available;
        match service.submit(intake_command(retryable)) {
            Ok(()) => {
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

    fn try_mount_pending_thread(&mut self, cx: &mut Context<Self>) {
        if self.pending_thread.is_none() {
            return;
        }
        self.retire_host(cx);
        if self.conversation_host.is_some() {
            return;
        }
        let Some(thread_id) = self.pending_thread.take() else {
            return;
        };
        self.selected_thread = Some(thread_id.clone());
        let Ok(host) = ConversationHost::mount(thread_id.clone(), ThemeMode::Dark, &mut *cx) else {
            self.set_failure(invalid_service_failure(), cx);
            return;
        };
        let subscription = cx.observe(&host, |application, host, cx| {
            application.collect_host_effects(&host, cx);
            application.pump_host_boundary(&host, cx);
        });
        self.conversation_host = Some(host.clone());
        drop(self.conversation_host_subscription.replace(subscription));
        suppress_conversation_tab_stops(&host, cx);
        self.collect_host_effects(&host, cx);
        self.pump_host_boundary(&host, cx);
        if self
            .pending_snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.thread_id() == &thread_id)
            && let Some(snapshot) = self.pending_snapshot.take()
        {
            self.dispatch_snapshot(&host, snapshot, cx);
        }
    }

    fn retire_host(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.conversation_host.clone() else {
            self.selected_thread = None;
            return;
        };
        self.pump_host_boundary(&host, cx);
        if self.conversation_effects.is_empty() && host.read(cx).total_pending_effect_count() == 0 {
            self.conversation_host = None;
            drop(self.conversation_host_subscription.take());
            self.selected_thread = None;
        }
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

    fn set_failure(&mut self, failure: ServiceFailure, cx: &mut Context<Self>) {
        self.state = NativeViewState::Failure(failure);
        cx.notify();
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
    let (heading, detail): (&str, String) = match state {
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
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .p(px(24.0))
        .rounded(px(8.0))
        .bg(theme.sidebar.sidebar.to_paint())
        .text_color(theme.colors.foreground.to_paint())
        .debug_selector(|| NATIVE_STATUS_SELECTOR.to_string())
        .child(heading)
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(detail),
        )
}

impl Render for NativeApplication {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let frame = ShellFrameStyle::resolve(self.theme);
        let mut header = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .pb(frame.surface_padding)
            .text_color(self.theme.colors.foreground.to_paint())
            .text_xl()
            .child(WINDOW_TITLE);
        if let Some(picker) = self.picker.clone() {
            header = header.child(picker);
        }

        let mut body = div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .items_center()
            .justify_center();
        if let Some(stage) = self.intake_stage {
            body = body.child(intake_status_panel(&self.theme, stage));
        } else if self.intake_failure_operation.is_some() {
            body = body.child(intake_failure_panel(
                &self.theme,
                self.intake_retry_available,
            ));
        } else if matches!(&self.state, NativeViewState::Ready) {
            if let Some(host) = self.conversation_host.clone() {
                body = body.child(host);
            } else {
                body = body.child(status_panel(&self.theme, &self.state));
            }
        } else {
            body = body.child(status_panel(&self.theme, &self.state));
        }

        div()
            .track_focus(&self.focus_handle)
            .key_context(NATIVE_KEY_CONTEXT)
            .on_action(|_: &NextTabStop, window, _| window.focus_next())
            .on_action(|_: &PreviousTabStop, window, _| window.focus_prev())
            .size_full()
            .flex()
            .flex_row()
            .bg(frame.window_background)
            .debug_selector(|| NATIVE_ROOT_SELECTOR.to_string())
            .child(shell_rail(frame).bg(self.theme.sidebar.sidebar.to_paint()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .min_h(px(0.0))
                    .pt(frame.surface_padding)
                    .pr(frame.surface_padding)
                    .pb(frame.surface_padding)
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(body),
            )
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
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap_2()
        .p(px(24.0))
        .rounded(px(8.0))
        .bg(theme.sidebar.sidebar.to_paint())
        .text_color(theme.colors.foreground.to_paint())
        .debug_selector(|| NATIVE_STATUS_SELECTOR.to_string())
        .child(heading)
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(detail),
        )
}

fn bind_native_actions(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("ctrl-q", Quit, None),
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

/// Launches the real native application window.
#[must_use]
pub fn run() -> ExitCode {
    let service = NativeTransportService::spawn().ok().map(Arc::new);
    let shutdown_started = Arc::new(AtomicBool::new(false));
    let launched = Rc::new(Cell::new(false));
    let launch_flag = Rc::clone(&launched);

    Application::new().run(move |cx: &mut App| {
        bind_native_actions(cx);

        let service_for_action = service.clone();
        let shutdown_for_action = Arc::clone(&shutdown_started);
        cx.on_action(move |_: &Quit, cx| {
            request_app_shutdown(cx, service_for_action.clone(), &shutdown_for_action);
        });

        let service_for_close = service.clone();
        let shutdown_for_close = Arc::clone(&shutdown_started);
        cx.on_window_closed(move |cx| {
            if cx.windows().is_empty() {
                request_app_shutdown(cx, service_for_close.clone(), &shutdown_for_close);
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(SURFACE_WIDTH), px(SURFACE_HEIGHT)), cx);
        let service_for_view = service.clone();
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(WINDOW_TITLE.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            move |window, cx| {
                let view =
                    cx.new(|view_cx| NativeApplication::new(service_for_view, window, view_cx));
                view.update(cx, NativeApplication::start_polling);
                view
            },
        );

        if opened.is_ok() {
            launch_flag.set(true);
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
    use super::{
        NativeApplication, NativeProjectIntakeOperation, NativeProjectIntakeStage, NativeViewState,
        PickerRoute, ServiceFailure, WINDOW_TITLE, intake_command, picker_route,
        project_options_from_listing, ready_membership_is_valid,
    };
    use crate::{
        conversation_delivery_machine::ConversationDeliveryEffect,
        conversation_host::{ConversationHost, ConversationHostEffect},
        conversation_state_machine::ConversationStateEffect,
        project_picker::{ProjectOption, ProjectPickerAction},
    };
    use artisan_domain::{
        ConversationCursor, ConversationSnapshot, DisplayName, ProjectId, ProjectListing,
        ProjectSummary, RootPath, ThreadId, ThreadListing, ThreadSummary, ThreadTitle, UnixMillis,
    };
    use artisan_ui::theme::ThemeMode;
    use gpui::TestAppContext;

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
                    projects,
                    project_id.clone(),
                    threads,
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
                    mismatched_projects,
                    ProjectId::parse("forge-p2").expect("project"),
                    mismatched_threads,
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

    #[test]
    fn production_title_is_the_native_title() {
        assert_eq!(WINDOW_TITLE, "Artisan");
        assert!(!WINDOW_TITLE.contains("phase"));
    }
}
