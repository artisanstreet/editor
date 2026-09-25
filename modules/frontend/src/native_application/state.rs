//! Native application state records and pure admission/admission-failure
//! policy helpers.
//!
//! Extracted verbatim from `native_application.rs` during the phase-1 module
//! split; visibility was widened to `pub(super)` for parent-owned state.

use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use crate::composer::{SubmissionBlocked, SubmissionToken};
use crate::native_transport_service::{
    CommandSendError, NativeTransportCommand, ServiceFailure, ServiceFailureCategory,
    ServiceFailureStage,
};
use crate::project_picker::{ProjectOption, ProjectPickerAction};
use artisan_domain::{ProjectId, ProjectListing, RequestId, ThreadId, ThreadListing};

#[cfg(test)]
#[derive(Clone)]
pub(super) struct NativeTestCommandSink {
    pub(super) commands: Rc<RefCell<Vec<NativeTransportCommand>>>,
    pub(super) outcomes: Rc<RefCell<VecDeque<Result<(), CommandSendError>>>>,
}

/// Application-facing state; every branch is honest about the native
/// milestone and contains no fixture catalog.
#[derive(Clone)]
pub(super) enum NativeViewState {
    Loading,
    EmptyProjects,
    LoadingThreads,
    EmptyThreads,
    Ready,
    Failure(ServiceFailure),
}

/// Application-owned identity for one admitted message queue.
///
/// Only identities: the Forge holds the payload from the moment it accepts
/// the message, and the composer keeps its own draft until the receipt. The
/// flight ends at the correlated receipt or failure; the connection hold
/// that covers it lives beside it on the application.
pub(super) struct NativeMessageFlight {
    pub(super) thread_id: ThreadId,
    pub(super) request_id: RequestId,
    pub(super) token: SubmissionToken,
}

#[derive(Clone, Copy)]
pub(super) struct NativeMessageFailure {
    pub(super) failure: ServiceFailure,
    pub(super) id: u64,
}
impl NativeMessageFailure {
    pub(super) fn new(failure: ServiceFailure) -> Self {
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
pub(super) struct ThreadSwitchFlight {
    pub(super) source_thread: ThreadId,
    pub(super) target_thread: Option<ThreadId>,
    pub(super) generation: u64,
    pub(super) carry_draft: bool,
    pub(super) phase: ThreadSwitchPhase,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ThreadSwitchPhase {
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

/// Mints a random `UUIDv7` request id (see [`RequestId::mint`]); ids stay
/// unique across Editor restarts and processes.
pub(super) fn mint_request_id(label: &str) -> Result<RequestId, ServiceFailure> {
    RequestId::mint(label).map_err(|_| invalid_service_failure())
}

pub(super) fn create_save_request_id() -> Result<RequestId, ServiceFailure> {
    mint_request_id("engine-save")
}

pub(super) fn create_message_request_id() -> Result<RequestId, ServiceFailure> {
    mint_request_id("native-message")
}

pub(super) fn submission_blocked_failure(blocked: SubmissionBlocked) -> Option<ServiceFailure> {
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
pub(super) enum PickerRoute {
    Select(ProjectId),
    BeginProjectIntake,
}

pub(super) fn project_options_from_listing(listing: &ProjectListing) -> Vec<ProjectOption> {
    listing
        .projects()
        .iter()
        .map(|project| ProjectOption {
            id: project.project_id.clone(),
            name: gpui::SharedString::from(project.display_name.as_str().to_owned()),
        })
        .collect()
}

pub(super) fn empty_thread_listing() -> ThreadListing {
    ThreadListing::new(Vec::new()).expect("an empty thread listing is always valid")
}

pub(super) fn ready_membership_is_valid(
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

pub(super) fn intake_command(retryable: bool) -> NativeTransportCommand {
    if retryable {
        NativeTransportCommand::RetryProjectIntake
    } else {
        NativeTransportCommand::BeginProjectIntake
    }
}

pub(super) fn picker_route(
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

pub(super) fn command_failure(error: CommandSendError) -> ServiceFailure {
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

pub(super) const fn invalid_service_failure() -> ServiceFailure {
    ServiceFailure {
        stage: ServiceFailureStage::Request,
        category: ServiceFailureCategory::Integrity,
    }
}
