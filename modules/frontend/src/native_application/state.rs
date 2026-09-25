//! Native application state records and pure admission/admission-failure
//! policy helpers.
//!
//! Extracted verbatim from `native_application.rs` during the phase-1 module
//! split; visibility was widened to `pub(super)` for parent-owned state.

use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use crate::composer::{SubmissionBlocked, SubmissionToken};
use crate::native_model_catalog::NativeModelCatalog;
use crate::native_transport_service::{
    CommandSendError, NativeTransportCommand, ServiceFailure, ServiceFailureCategory,
    ServiceFailureStage,
};
use crate::project_picker::{ProjectOption, ProjectPickerAction};
use artisan_domain::{
    MessageId, ProjectId, ProjectListing, QueueMessagePayload, RequestId, ThreadId, ThreadListing,
};

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
/// The body is intentionally retained here until the application observes a
/// correlated terminal result. This type owns message text and therefore
/// implements neither `Debug` nor `Display`.
pub(super) struct NativeMessageFlight {
    pub(super) thread_id: ThreadId,
    pub(super) request_id: RequestId,
    pub(super) payload: QueueMessagePayload,
    /// Observed-run steer target named at send time, preserved verbatim
    /// for the same-identity retry. `None` is a fresh send.
    pub(super) steer_target: Option<artisan_domain::SteerTarget>,
    /// Validated routed engine display label captured at send from the
    /// authoritative config (never the picker). Preserved verbatim for the
    /// retry and the Waiting narration; `None` renders the generic fallback.
    pub(super) engine_label: Option<String>,
    pub(super) token: SubmissionToken,
}

/// One explicit new-chat recovery over an exact terminally failed dispatch.
///
/// The recovery carries only identities until the recalled payload arrives:
/// the old thread/message/original-request triple that owns the failed
/// prompt, the exact policy displayed for the old thread, and the created
/// thread once intake resolves it. The prompt moves to the newly created
/// thread as an unsent draft through the existing recalled-payload restore;
/// the old thread keeps its history, the failed row stays terminal, and
/// nothing is ever autosent. The policy and thread identities own no message
/// text and are safe to `Debug`.
#[derive(Clone)]
pub(super) struct PendingFailedRecovery {
    pub(super) old_thread: ThreadId,
    pub(super) message_id: MessageId,
    pub(super) original_request_id: RequestId,
    pub(super) policy: Option<crate::native_model_catalog::NativeModelPolicy>,
    pub(super) new_thread: Option<ThreadId>,
    pub(super) recalled: bool,
}

/// Outcome of first-send admission for a thread without a persisted engine
/// configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum FirstSendAdmission {
    /// The thread is already configured; continue the send.
    Proceed,
    /// The send was held for a save, adopted, blocked, or suppressed; the
    /// gate already synced and notified.
    Held,
}

/// Application-owned identity for one explicitly retryable message
/// queue. This type owns message text and therefore implements neither
/// `Debug` nor `Display`.
pub(super) struct NativeMessageRetry {
    pub(super) thread_id: ThreadId,
    pub(super) request_id: RequestId,
    pub(super) payload: QueueMessagePayload,
    /// Original steer target from the failed send. A retry replays it
    /// verbatim and never re-resolves the current live run.
    pub(super) steer_target: Option<artisan_domain::SteerTarget>,
    /// Original engine label from the failed send. A retry replays it
    /// verbatim and never re-resolves a changed picker after the send.
    pub(super) engine_label: Option<String>,
    pub(super) draft_matches: bool,
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

/// Loads the Forge-published scope-free catalog snapshot, if present.
///
/// The Forge writes `<home>/readiness/model-catalog.json` (live discovery
/// only) next to its readiness receipt. Surfaces without a thread-scoped
/// runtime read (the home picker) use this snapshot when it is available and
/// otherwise show no models. Snapshots from an older harness revision fail
/// wire validation, so rows published before an update never resurface.
#[cfg(test)]
pub(super) fn scope_free_catalog_snapshot() -> Option<NativeModelCatalog> {
    None
}

#[cfg(not(test))]
pub(super) fn scope_free_catalog_snapshot() -> Option<NativeModelCatalog> {
    let layout = artisan_editor_cli::paths::Layout::discover().ok()?;
    let path = layout.root.join("readiness").join("model-catalog.json");
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > 16 * 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    artisan_protocol::CatalogSnapshotWire::new(bytes)
        .ok()?
        .decoded()
        .ok()
}

/// Mints a random UUIDv7 request id (see [`RequestId::mint`]); ids stay
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
