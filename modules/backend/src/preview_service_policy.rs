//! Dependency-free policy for composing preview service repository actions.
//!
//! This module owns the deterministic boundary between preview service calls
//! and the repository. It validates target identifiers and describes the
//! repository call to make; it does not execute storage, network, URL probing,
//! asynchronous work, or runtime coordination.

pub const MIN_PREVIEW_TARGET_ID_SCALARS: usize = 1;
pub const MAX_PREVIEW_TARGET_ID_SCALARS: usize = 256;

/// The only service-level validation failure produced by this policy.
///
/// The unit variant is intentional: invalid identifiers do not get copied
/// into an error payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewServiceError {
    InvalidTargetId,
}

pub type PreviewRepositoryError = PreviewServiceError;
pub type PreviewTargetIdError = PreviewServiceError;

/// Validates a preview target identifier by Unicode scalar value count.
///
/// This follows the source service's string length boundary: it does not trim
/// or normalize the value, and `char::count` deliberately counts scalar values
/// rather than UTF-8 bytes or grapheme clusters.
pub fn validate_preview_target_id(target_id: &str) -> Result<(), PreviewServiceError> {
    let scalar_count = target_id.chars().count();
    if (MIN_PREVIEW_TARGET_ID_SCALARS..=MAX_PREVIEW_TARGET_ID_SCALARS).contains(&scalar_count) {
        Ok(())
    } else {
        Err(PreviewServiceError::InvalidTargetId)
    }
}

pub fn validate_target_id(target_id: &str) -> Result<(), PreviewServiceError> {
    validate_preview_target_id(target_id)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreviewSource {
    Process { process_id: String },
    Terminal { terminal_id: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewTargetState {
    Registered,
    Healthy,
    Unhealthy,
    Stopped,
    Removed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewTargetLaunchState {
    Idle,
    Launching,
    Launched,
    Unavailable,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewTargetUpdateAction {
    Launch,
    Probe,
    Remove,
    State,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewInspectionState {
    Open,
    Closed,
    Abandoned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewInspectionReconnectState {
    Connected,
    Reconnecting,
    Unavailable,
    Error,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewInspectionAction {
    Close,
    Open,
    Reconnect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewDispatchLeaseKind {
    Launch,
    Probe,
    InspectionOpen,
    InspectionHealth,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewTargetProjection {
    pub target_id: String,
    pub thread_id: String,
    pub project_id: String,
    pub workspace_id: String,
    pub url: String,
    pub port: u16,
    pub routes_json: String,
    pub source: Option<PreviewSource>,
    pub state: PreviewTargetState,
    pub launch_state: PreviewTargetLaunchState,
    pub last_error: Option<String>,
    pub health_json: Option<String>,
    pub journal_sequence: u64,
    pub created_at: String,
    pub updated_at: String,
    pub removed_at: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewInspectionProjection {
    pub session_id: String,
    pub target_id: String,
    pub thread_id: String,
    pub connector_id: String,
    pub state: PreviewInspectionState,
    pub reconnect_state: PreviewInspectionReconnectState,
    pub last_error: Option<String>,
    pub journal_sequence: u64,
    pub opened_at: String,
    pub closed_at: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewDispatchLease {
    pub acquired_at: String,
    pub expires_at: String,
    pub kind: PreviewDispatchLeaseKind,
    pub lease_id: String,
    pub owner_instance_id: String,
    pub session_id: Option<String>,
    pub target_id: Option<String>,
    pub thread_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewRegisterCommand {
    pub message_id: String,
    pub port: u16,
    pub project_id: String,
    pub routes: Option<Vec<String>>,
    pub source: Option<PreviewSource>,
    pub target_id: String,
    pub thread_id: String,
    pub url: String,
    pub workspace_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewTargetUpdateCommand {
    pub action: PreviewTargetUpdateAction,
    pub health_json: Option<String>,
    pub last_error: Option<String>,
    pub launch_state: Option<PreviewTargetLaunchState>,
    pub message_id: String,
    pub state: Option<PreviewTargetState>,
    pub target_id: String,
    pub thread_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewInspectionCommand {
    pub action: PreviewInspectionAction,
    pub connector_id: Option<String>,
    pub last_error: Option<String>,
    pub message_id: String,
    pub reconnect_state: Option<PreviewInspectionReconnectState>,
    pub session_id: String,
    pub target_id: Option<String>,
    pub thread_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewDispatchLeaseInput {
    pub kind: PreviewDispatchLeaseKind,
    pub session_id: Option<String>,
    pub target_id: Option<String>,
    pub thread_id: String,
}

/// Typed repository calls emitted by [`PreviewServicePolicy`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreviewServiceAction {
    GetTarget {
        target_id: String,
    },
    ListTargets {
        workspace_id: Option<String>,
    },
    ValidateLocalPreviewUrl {
        url: String,
    },
    Register {
        input: PreviewRegisterCommand,
    },
    ReplayTargetUpdate {
        input: PreviewTargetUpdateCommand,
    },
    UpdateTarget {
        input: PreviewTargetUpdateCommand,
        dispatch_lease_id: Option<String>,
    },
    UpdateInspection {
        input: PreviewInspectionCommand,
        dispatch_lease_id: Option<String>,
    },
    RecoverInspections,
    AcquireDispatchLease {
        input: PreviewDispatchLeaseInput,
    },
    ReleaseDispatchLease {
        lease: PreviewDispatchLease,
    },
    RenewDispatchLease {
        lease: PreviewDispatchLease,
    },
    RecoverDispatchLeases,
}

pub type PreviewRepositoryAction = PreviewServiceAction;

/// Result shape of the replay repository call.
pub type ReplayTargetUpdateResult = Option<PreviewTargetProjection>;

/// The result contract associated with a typed repository action.
///
/// In particular, replay is `OptionalTarget`, while an ordinary target update
/// is `Target`; keeping this distinction at the policy boundary prevents a
/// replay miss from being represented as an ordinary update result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewRepositoryResultKind {
    Target,
    Targets,
    CanonicalUrl,
    OptionalTarget,
    Inspection,
    Inspections,
    DispatchLease,
    Unit,
    DispatchLeases,
}

impl PreviewServiceAction {
    #[must_use]
    pub const fn result_kind(&self) -> PreviewRepositoryResultKind {
        match self {
            Self::GetTarget { .. } | Self::Register { .. } | Self::UpdateTarget { .. } => {
                PreviewRepositoryResultKind::Target
            }
            Self::ListTargets { .. } => PreviewRepositoryResultKind::Targets,
            Self::ValidateLocalPreviewUrl { .. } => PreviewRepositoryResultKind::CanonicalUrl,
            Self::ReplayTargetUpdate { .. } => PreviewRepositoryResultKind::OptionalTarget,
            Self::UpdateInspection { .. } => PreviewRepositoryResultKind::Inspection,
            Self::RecoverInspections => PreviewRepositoryResultKind::Inspections,
            Self::AcquireDispatchLease { .. } | Self::RenewDispatchLease { .. } => {
                PreviewRepositoryResultKind::DispatchLease
            }
            Self::ReleaseDispatchLease { .. } => PreviewRepositoryResultKind::Unit,
            Self::RecoverDispatchLeases => PreviewRepositoryResultKind::DispatchLeases,
        }
    }
}

/// Stateless composition boundary for preview service calls.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PreviewServicePolicy;

impl PreviewServicePolicy {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    pub fn get<T>(&self, target_id: T) -> Result<PreviewServiceAction, PreviewServiceError>
    where
        T: Into<String>,
    {
        let target_id = target_id.into();
        validate_preview_target_id(&target_id)?;
        Ok(PreviewServiceAction::GetTarget { target_id })
    }

    #[must_use]
    pub fn list(&self, workspace_id: Option<&str>) -> PreviewServiceAction {
        PreviewServiceAction::ListTargets {
            workspace_id: workspace_id.map(str::to_owned),
        }
    }

    #[must_use]
    pub fn validate_target_url<T>(&self, url: T) -> PreviewServiceAction
    where
        T: Into<String>,
    {
        PreviewServiceAction::ValidateLocalPreviewUrl { url: url.into() }
    }

    #[must_use]
    pub fn register(&self, input: PreviewRegisterCommand) -> PreviewServiceAction {
        PreviewServiceAction::Register { input }
    }

    #[must_use]
    pub fn replay_target_update(&self, input: PreviewTargetUpdateCommand) -> PreviewServiceAction {
        PreviewServiceAction::ReplayTargetUpdate { input }
    }

    #[must_use]
    pub fn update_target(
        &self,
        input: PreviewTargetUpdateCommand,
        dispatch_lease_id: Option<&str>,
    ) -> PreviewServiceAction {
        PreviewServiceAction::UpdateTarget {
            input,
            dispatch_lease_id: dispatch_lease_id.map(str::to_owned),
        }
    }

    #[must_use]
    pub fn update_inspection(
        &self,
        input: PreviewInspectionCommand,
        dispatch_lease_id: Option<&str>,
    ) -> PreviewServiceAction {
        PreviewServiceAction::UpdateInspection {
            input,
            dispatch_lease_id: dispatch_lease_id.map(str::to_owned),
        }
    }

    #[must_use]
    pub fn recover_inspections(&self) -> PreviewServiceAction {
        PreviewServiceAction::RecoverInspections
    }

    #[must_use]
    pub fn acquire_dispatch_lease(&self, input: PreviewDispatchLeaseInput) -> PreviewServiceAction {
        PreviewServiceAction::AcquireDispatchLease { input }
    }

    #[must_use]
    pub fn release_dispatch_lease(&self, lease: PreviewDispatchLease) -> PreviewServiceAction {
        PreviewServiceAction::ReleaseDispatchLease { lease }
    }

    #[must_use]
    pub fn renew_dispatch_lease(&self, lease: PreviewDispatchLease) -> PreviewServiceAction {
        PreviewServiceAction::RenewDispatchLease { lease }
    }

    #[must_use]
    pub fn recover_dispatch_leases(&self) -> PreviewServiceAction {
        PreviewServiceAction::RecoverDispatchLeases
    }
}

pub fn get<T>(target_id: T) -> Result<PreviewServiceAction, PreviewServiceError>
where
    T: Into<String>,
{
    PreviewServicePolicy::new().get(target_id)
}

#[must_use]
pub fn list(workspace_id: Option<&str>) -> PreviewServiceAction {
    PreviewServicePolicy::new().list(workspace_id)
}

#[must_use]
pub fn validate_target_url<T>(url: T) -> PreviewServiceAction
where
    T: Into<String>,
{
    PreviewServicePolicy::new().validate_target_url(url)
}

#[must_use]
pub fn register(input: PreviewRegisterCommand) -> PreviewServiceAction {
    PreviewServicePolicy::new().register(input)
}

#[must_use]
pub fn replay_target_update(input: PreviewTargetUpdateCommand) -> PreviewServiceAction {
    PreviewServicePolicy::new().replay_target_update(input)
}

#[must_use]
pub fn update_target(
    input: PreviewTargetUpdateCommand,
    dispatch_lease_id: Option<&str>,
) -> PreviewServiceAction {
    PreviewServicePolicy::new().update_target(input, dispatch_lease_id)
}

#[must_use]
pub fn update_inspection(
    input: PreviewInspectionCommand,
    dispatch_lease_id: Option<&str>,
) -> PreviewServiceAction {
    PreviewServicePolicy::new().update_inspection(input, dispatch_lease_id)
}

#[must_use]
pub fn recover_inspections() -> PreviewServiceAction {
    PreviewServicePolicy::new().recover_inspections()
}

#[must_use]
pub fn acquire_dispatch_lease(input: PreviewDispatchLeaseInput) -> PreviewServiceAction {
    PreviewServicePolicy::new().acquire_dispatch_lease(input)
}

#[must_use]
pub fn release_dispatch_lease(lease: PreviewDispatchLease) -> PreviewServiceAction {
    PreviewServicePolicy::new().release_dispatch_lease(lease)
}

#[must_use]
pub fn renew_dispatch_lease(lease: PreviewDispatchLease) -> PreviewServiceAction {
    PreviewServicePolicy::new().renew_dispatch_lease(lease)
}

#[must_use]
pub fn recover_dispatch_leases() -> PreviewServiceAction {
    PreviewServicePolicy::new().recover_dispatch_leases()
}
