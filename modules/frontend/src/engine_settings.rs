//! Thread-bound engine settings state and validation.
//!
//! The controller owns no runtime or GPUI entity. It models the
//! application-visible lifecycle for one selected real thread and
//! retains only redacted diagnostics.

#![forbid(unsafe_code)]

use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, EngineAgentId, EngineConfigError, EngineConfigReason,
    EngineConfigRevision, EngineModelId, EnginePermissionPolicy, EngineProfileId, EngineRouteId,
    EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput, EngineSelection,
    EngineVariantId, FilesystemAccess, FiniteMillis, NetworkAccess, OpenCode2Selection,
    PermissionId, ThreadId, WebSearchAccess,
};
use artisan_protocol::{
    RegisteredEngineProfilesResult, SetThreadEngineConfigResult, ThreadEngineSettingsResult,
};

use crate::native_transport_service::{
    ServiceFailure, ServiceFailureCategory, ServiceFailureStage, SettingsLoadGeneration,
};

/// Stable field order for the explicit clipboard document.
///
/// The template deliberately leaves every value empty. In particular, no
/// profile, model, route, policy, or runtime value is inferred by the
/// clipboard interaction.
pub const MANUAL_CONFIGURATION_KEYS: [&str; 24] = [
    "profile_id",
    "model_id",
    "route_id",
    "variant_id",
    "permission_id",
    "agent_id",
    "approval",
    "filesystem",
    "network",
    "web_search",
    "attempt_budget",
    "readiness_budget",
    "health_budget",
    "prompt_budget",
    "stream_budget",
    "close_budget",
    "max_json_body_bytes",
    "max_sse_line_bytes",
    "max_sse_event_bytes",
    "max_readiness_line_bytes",
    "max_header_count",
    "max_http_buffer_bytes",
    "max_stderr_bytes",
    "observation_capacity",
];

/// Finite byte bound for one clipboard configuration document.
pub const MAX_MANUAL_CONFIGURATION_BYTES: usize = 16 * 1024;

/// Finite line bound for one clipboard configuration document.
pub const MAX_MANUAL_CONFIGURATION_LINES: usize = MANUAL_CONFIGURATION_KEYS.len();

/// Operation whose redacted failure is currently visible in the settings UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineSettingsFailureOperation {
    /// The certified profile catalogue could not be admitted or read.
    Registry,
    /// The selected thread's authoritative settings could not be read.
    SettingsRead,
    /// The selected thread's durable save failed.
    Save,
    /// A clipboard document or local draft value was rejected.
    Input,
}

/// Visible lifecycle for the engine-settings section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineSettingsStatus {
    /// No thread selected.
    Unselected,
    /// Awaiting authoritative settings and possibly registry.
    Loading,
    /// Registry is absent on this host.
    RegistryMissing,
    /// Registry is present but contains no certified profile.
    RegistryPresentEmpty,
    /// Authoritatively unconfigured.
    Unconfigured,
    /// Authoritatively configured and no local edits.
    Ready,
    /// Local draft differs from authoritative.
    Dirty,
    /// Save in flight.
    Saving,
    /// Conflict detected; one authoritative reload is required.
    ConflictRefreshing,
    /// Redacted failure.
    Failure,
}

/// Registry view exposed to the UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryView {
    /// Not yet loaded.
    Loading,
    /// No registry file exists.
    Missing,
    /// Registry exists but is empty.
    PresentEmpty,
    /// Registry exists with ordered profile ids.
    Present(Vec<EngineProfileId>),
}

/// Raw string draft for every required `EngineRunConfig` field.
///
/// Every field starts empty. No default is ever synthesized.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EngineSettingsDraft {
    pub profile_id: String,
    pub model_id: String,
    pub route_id: String,
    /// Empty means no variant.
    pub variant_id: String,
    pub permission_id: String,
    pub agent_id: String,
    /// Valid spellings: `never`, `on_request`, `always`.
    pub approval: String,
    /// Valid spellings: `none`, `workspace`, `host`.
    pub filesystem: String,
    /// Valid spellings: `disabled`, `enabled`.
    pub network: String,
    /// Valid spellings: `disabled`, `enabled`.
    pub web_search: String,
    pub attempt_budget: String,
    pub readiness_budget: String,
    pub health_budget: String,
    pub prompt_budget: String,
    pub stream_budget: String,
    pub close_budget: String,
    pub max_json_body_bytes: String,
    pub max_sse_line_bytes: String,
    pub max_sse_event_bytes: String,
    pub max_readiness_line_bytes: String,
    pub max_header_count: String,
    pub max_http_buffer_bytes: String,
    pub max_stderr_bytes: String,
    pub observation_capacity: String,
}

fn manual_configuration_error(
    field: &'static str,
    reason: EngineConfigReason,
) -> EngineConfigError {
    EngineConfigError::new(field, reason)
}

fn manual_field_index(key: &str) -> Option<usize> {
    MANUAL_CONFIGURATION_KEYS
        .iter()
        .position(|known| *known == key)
}

/// Returns the stable empty-value clipboard document used by the settings
/// surface. The document contains one exact `key=` line for every field and
/// never copies a value from the current or authoritative configuration.
#[must_use]
pub fn manual_configuration_template() -> String {
    let mut template = String::new();
    for key in MANUAL_CONFIGURATION_KEYS {
        template.push_str(key);
        template.push_str("=\n");
    }
    template
}

/// Parses one complete manual settings document without retaining the source
/// text in either success or failure state.
///
/// # Errors
///
/// Returns a bounded [`EngineConfigError`] for an oversized document, a
/// malformed/unknown/duplicate line, or a missing required field. The error
/// contains only a stable field label and finite reason category.
pub fn parse_manual_configuration(
    document: &str,
) -> Result<EngineSettingsDraft, EngineConfigError> {
    if document.len() > MAX_MANUAL_CONFIGURATION_BYTES {
        return Err(manual_configuration_error(
            "document",
            EngineConfigReason::OutOfRange,
        ));
    }

    let mut draft = EngineSettingsDraft::default();
    let mut seen = [false; MANUAL_CONFIGURATION_KEYS.len()];
    let mut line_count = 0usize;
    for line in document.lines() {
        line_count = line_count.saturating_add(1);
        if line_count > MAX_MANUAL_CONFIGURATION_LINES {
            return Err(manual_configuration_error(
                "document",
                EngineConfigReason::OutOfRange,
            ));
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(manual_configuration_error(
                "configuration",
                EngineConfigReason::InvalidIdentifier,
            ));
        };
        let Some(index) = manual_field_index(key) else {
            return Err(manual_configuration_error(
                "configuration",
                EngineConfigReason::Unsupported,
            ));
        };
        if seen[index] {
            return Err(manual_configuration_error(
                MANUAL_CONFIGURATION_KEYS[index],
                EngineConfigReason::Inconsistent,
            ));
        }
        if value.contains('=') {
            return Err(manual_configuration_error(
                MANUAL_CONFIGURATION_KEYS[index],
                EngineConfigReason::InvalidIdentifier,
            ));
        }
        seen[index] = true;
        match key {
            "profile_id" => value.clone_into(&mut draft.profile_id),
            "model_id" => value.clone_into(&mut draft.model_id),
            "route_id" => value.clone_into(&mut draft.route_id),
            "variant_id" => value.clone_into(&mut draft.variant_id),
            "permission_id" => value.clone_into(&mut draft.permission_id),
            "agent_id" => value.clone_into(&mut draft.agent_id),
            "approval" => value.clone_into(&mut draft.approval),
            "filesystem" => value.clone_into(&mut draft.filesystem),
            "network" => value.clone_into(&mut draft.network),
            "web_search" => value.clone_into(&mut draft.web_search),
            "attempt_budget" => value.clone_into(&mut draft.attempt_budget),
            "readiness_budget" => value.clone_into(&mut draft.readiness_budget),
            "health_budget" => value.clone_into(&mut draft.health_budget),
            "prompt_budget" => value.clone_into(&mut draft.prompt_budget),
            "stream_budget" => value.clone_into(&mut draft.stream_budget),
            "close_budget" => value.clone_into(&mut draft.close_budget),
            "max_json_body_bytes" => value.clone_into(&mut draft.max_json_body_bytes),
            "max_sse_line_bytes" => value.clone_into(&mut draft.max_sse_line_bytes),
            "max_sse_event_bytes" => value.clone_into(&mut draft.max_sse_event_bytes),
            "max_readiness_line_bytes" => value.clone_into(&mut draft.max_readiness_line_bytes),
            "max_header_count" => value.clone_into(&mut draft.max_header_count),
            "max_http_buffer_bytes" => value.clone_into(&mut draft.max_http_buffer_bytes),
            "max_stderr_bytes" => value.clone_into(&mut draft.max_stderr_bytes),
            "observation_capacity" => value.clone_into(&mut draft.observation_capacity),
            _ => unreachable!("manual field index and assignment table diverged"),
        }
    }

    if let Some((index, _)) = seen.iter().enumerate().find(|(_, present)| !**present) {
        return Err(manual_configuration_error(
            MANUAL_CONFIGURATION_KEYS[index],
            EngineConfigReason::InvalidIdentifier,
        ));
    }
    Ok(draft)
}

fn parse_variant_id(value: &str) -> Result<Option<EngineVariantId>, EngineConfigError> {
    if value.is_empty() {
        return Ok(None);
    }
    EngineVariantId::parse(value.to_owned())
        .map(Some)
        .map_err(|_| {
            manual_configuration_error("variant_id", EngineConfigReason::InvalidIdentifier)
        })
}

fn parse_approval(value: &str) -> Result<ApprovalMode, EngineConfigError> {
    match value {
        "never" => Ok(ApprovalMode::Never),
        "on_request" => Ok(ApprovalMode::OnRequest),
        "always" => Ok(ApprovalMode::Always),
        _ => Err(manual_configuration_error(
            "approval",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn parse_filesystem(value: &str) -> Result<FilesystemAccess, EngineConfigError> {
    match value {
        "none" => Ok(FilesystemAccess::None),
        "workspace" => Ok(FilesystemAccess::Workspace),
        "host" => Ok(FilesystemAccess::Host),
        _ => Err(manual_configuration_error(
            "filesystem",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn parse_network(value: &str) -> Result<NetworkAccess, EngineConfigError> {
    match value {
        "disabled" => Ok(NetworkAccess::Disabled),
        "enabled" => Ok(NetworkAccess::Enabled),
        _ => Err(manual_configuration_error(
            "network",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn parse_web_search(value: &str) -> Result<WebSearchAccess, EngineConfigError> {
    match value {
        "disabled" => Ok(WebSearchAccess::Disabled),
        "enabled" => Ok(WebSearchAccess::Enabled),
        _ => Err(manual_configuration_error(
            "web_search",
            EngineConfigReason::Unsupported,
        )),
    }
}

fn parse_millis(field: &'static str, value: &str) -> Result<FiniteMillis, EngineConfigError> {
    let value = value
        .parse::<u64>()
        .map_err(|_| manual_configuration_error(field, EngineConfigReason::InvalidIdentifier))?;
    FiniteMillis::new(value).map_err(|error| manual_configuration_error(field, error.reason()))
}

fn parse_bytes(field: &'static str, value: &str) -> Result<ByteLimit, EngineConfigError> {
    let value = value
        .parse::<u64>()
        .map_err(|_| manual_configuration_error(field, EngineConfigReason::InvalidIdentifier))?;
    ByteLimit::new(value).map_err(|error| manual_configuration_error(field, error.reason()))
}

fn parse_count(field: &'static str, value: &str) -> Result<CountLimit, EngineConfigError> {
    let value = value
        .parse::<u64>()
        .map_err(|_| manual_configuration_error(field, EngineConfigReason::InvalidIdentifier))?;
    CountLimit::new(value).map_err(|error| manual_configuration_error(field, error.reason()))
}

impl EngineSettingsDraft {
    /// Builds a draft reflecting an authoritative config.
    #[must_use]
    pub fn from_config(config: &EngineRunConfig) -> Self {
        let selection = config.selection().as_opencode2();
        let runtime = config.runtime();
        let permission = selection.permission();
        Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model_id: selection.model_id().as_str().to_owned(),
            route_id: selection.route_id().as_str().to_owned(),
            variant_id: selection
                .variant_id()
                .map_or_else(String::new, |id| id.as_str().to_owned()),
            permission_id: permission.permission_id().as_str().to_owned(),
            agent_id: permission.agent_id().as_str().to_owned(),
            approval: permission.approval().as_str().to_owned(),
            filesystem: permission.filesystem().as_str().to_owned(),
            network: permission.network().as_str().to_owned(),
            web_search: permission.web_search().as_str().to_owned(),
            attempt_budget: runtime.attempt_budget().get().to_string(),
            readiness_budget: runtime.readiness_budget().get().to_string(),
            health_budget: runtime.health_budget().get().to_string(),
            prompt_budget: runtime.prompt_budget().get().to_string(),
            stream_budget: runtime.stream_budget().get().to_string(),
            close_budget: runtime.close_budget().get().to_string(),
            max_json_body_bytes: runtime.max_json_body_bytes().get().to_string(),
            max_sse_line_bytes: runtime.max_sse_line_bytes().get().to_string(),
            max_sse_event_bytes: runtime.max_sse_event_bytes().get().to_string(),
            max_readiness_line_bytes: runtime.max_readiness_line_bytes().get().to_string(),
            max_header_count: runtime.max_header_count().get().to_string(),
            max_http_buffer_bytes: runtime.max_http_buffer_bytes().get().to_string(),
            max_stderr_bytes: runtime.max_stderr_bytes().get().to_string(),
            observation_capacity: runtime.observation_capacity().get().to_string(),
        }
    }

    fn parse_registered_profile(
        &self,
        registry: Option<&RegisteredEngineProfilesResult>,
    ) -> Result<EngineProfileId, EngineConfigError> {
        let profile_id = EngineProfileId::parse(self.profile_id.clone()).map_err(|_| {
            manual_configuration_error("profile_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let Some(RegisteredEngineProfilesResult::RegistryPresent { profile_ids }) = registry else {
            return Err(manual_configuration_error(
                "profile_id",
                EngineConfigReason::InvalidIdentifier,
            ));
        };
        if !profile_ids
            .iter()
            .any(|id| id.as_str() == profile_id.as_str())
        {
            return Err(manual_configuration_error(
                "profile_id",
                EngineConfigReason::InvalidIdentifier,
            ));
        }
        Ok(profile_id)
    }

    fn build_selection(
        &self,
        registry: Option<&RegisteredEngineProfilesResult>,
    ) -> Result<EngineSelection, EngineConfigError> {
        let profile_id = self.parse_registered_profile(registry)?;
        let model_id = EngineModelId::parse(self.model_id.clone()).map_err(|_| {
            manual_configuration_error("model_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let route_id = EngineRouteId::parse(self.route_id.clone()).map_err(|_| {
            manual_configuration_error("route_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let variant_id = parse_variant_id(&self.variant_id)?;
        let permission_id = PermissionId::parse(self.permission_id.clone()).map_err(|_| {
            manual_configuration_error("permission_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let agent_id = EngineAgentId::parse(self.agent_id.clone()).map_err(|_| {
            manual_configuration_error("agent_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let approval = parse_approval(&self.approval)?;
        let filesystem = parse_filesystem(&self.filesystem)?;
        let network = parse_network(&self.network)?;
        let web_search = parse_web_search(&self.web_search)?;
        let permission = EnginePermissionPolicy::new(
            permission_id,
            agent_id,
            approval,
            filesystem,
            network,
            web_search,
        );
        Ok(EngineSelection::OpenCode2(OpenCode2Selection::new(
            profile_id, model_id, route_id, variant_id, permission,
        )))
    }

    fn build_runtime_controls(&self) -> Result<EngineRuntimeControls, EngineConfigError> {
        EngineRuntimeControls::new(EngineRuntimeControlsInput {
            attempt_budget: parse_millis("attempt_budget", &self.attempt_budget)?,
            readiness_budget: parse_millis("readiness_budget", &self.readiness_budget)?,
            health_budget: parse_millis("health_budget", &self.health_budget)?,
            prompt_budget: parse_millis("prompt_budget", &self.prompt_budget)?,
            stream_budget: parse_millis("stream_budget", &self.stream_budget)?,
            close_budget: parse_millis("close_budget", &self.close_budget)?,
            max_json_body_bytes: parse_bytes("max_json_body_bytes", &self.max_json_body_bytes)?,
            max_sse_line_bytes: parse_bytes("max_sse_line_bytes", &self.max_sse_line_bytes)?,
            max_sse_event_bytes: parse_bytes("max_sse_event_bytes", &self.max_sse_event_bytes)?,
            max_readiness_line_bytes: parse_bytes(
                "max_readiness_line_bytes",
                &self.max_readiness_line_bytes,
            )?,
            max_header_count: parse_count("max_header_count", &self.max_header_count)?,
            max_http_buffer_bytes: parse_bytes(
                "max_http_buffer_bytes",
                &self.max_http_buffer_bytes,
            )?,
            max_stderr_bytes: parse_bytes("max_stderr_bytes", &self.max_stderr_bytes)?,
            observation_capacity: parse_count("observation_capacity", &self.observation_capacity)?,
        })
    }

    /// Attempts to build a complete validated `EngineRunConfig`.
    ///
    /// # Errors
    ///
    /// Returns the first bounded domain validation failure without exposing
    /// the rejected value.
    pub fn build_config(
        &self,
        registry: Option<&RegisteredEngineProfilesResult>,
    ) -> Result<EngineRunConfig, EngineConfigError> {
        let selection = self.build_selection(registry)?;
        let runtime = self.build_runtime_controls()?;
        Ok(EngineRunConfig::new(selection, runtime))
    }
}

/// Application-owned controller for thread-bound engine settings.
struct PendingSave {
    thread_id: ThreadId,
    request_id: artisan_domain::RequestId,
    retained: EngineRunConfig,
}

#[derive(Clone, Copy, Default)]
struct RegistryLoadFlags {
    loading: bool,
    needed: bool,
}

#[derive(Clone, Copy, Default)]
struct SettingsLoadFlags {
    needed: bool,
    conflict_refreshing: bool,
}

pub struct EngineSettingsController {
    selected_thread: Option<ThreadId>,
    registry: Option<RegisteredEngineProfilesResult>,
    registry_load: RegistryLoadFlags,
    settings: Option<ThreadEngineSettingsResult>,
    loading_thread: Option<ThreadId>,
    active_settings_generation: Option<SettingsLoadGeneration>,
    next_settings_load_generation: Option<SettingsLoadGeneration>,
    settings_load: SettingsLoadFlags,
    draft: EngineSettingsDraft,
    authoritative_revision: Option<EngineConfigRevision>,
    authoritative_config: Option<EngineRunConfig>,
    pending_save: Option<PendingSave>,
    registry_failure: Option<ServiceFailure>,
    settings_failure: Option<ServiceFailure>,
    save_failure: Option<ServiceFailure>,
    input_error: Option<EngineConfigError>,
    needs_settings_reload: Option<ThreadId>,
}

impl EngineSettingsController {
    /// Creates an empty controller with no selection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            selected_thread: None,
            registry: None,
            registry_load: RegistryLoadFlags::default(),
            settings: None,
            loading_thread: None,
            active_settings_generation: None,
            next_settings_load_generation: None,
            settings_load: SettingsLoadFlags::default(),
            draft: EngineSettingsDraft::default(),
            authoritative_revision: None,
            authoritative_config: None,
            pending_save: None,
            registry_failure: None,
            settings_failure: None,
            save_failure: None,
            input_error: None,
            needs_settings_reload: None,
        }
    }

    /// Returns the selected thread.
    #[must_use]
    pub fn selected_thread(&self) -> Option<&ThreadId> {
        self.selected_thread.as_ref()
    }

    /// Returns the registry view.
    #[must_use]
    pub fn registry_view(&self) -> RegistryView {
        if self.registry_load.loading {
            return RegistryView::Loading;
        }
        match &self.registry {
            None => RegistryView::Loading,
            Some(RegisteredEngineProfilesResult::RegistryMissing) => RegistryView::Missing,
            Some(RegisteredEngineProfilesResult::RegistryPresent { profile_ids })
                if profile_ids.is_empty() =>
            {
                RegistryView::PresentEmpty
            }
            Some(RegisteredEngineProfilesResult::RegistryPresent { profile_ids }) => {
                RegistryView::Present(profile_ids.clone())
            }
        }
    }

    /// Returns the draft for UI binding.
    #[must_use]
    pub fn draft(&self) -> &EngineSettingsDraft {
        &self.draft
    }

    /// Returns a mutable draft for explicit manual editing.
    pub fn draft_mut(&mut self) -> &mut EngineSettingsDraft {
        &mut self.draft
    }

    /// Returns the authoritative settings if loaded.
    #[must_use]
    pub fn authoritative_settings(&self) -> Option<&ThreadEngineSettingsResult> {
        self.settings.as_ref()
    }

    /// Returns the exact generation accepted for the active settings load.
    #[must_use]
    pub const fn active_settings_generation(&self) -> Option<SettingsLoadGeneration> {
        self.active_settings_generation
    }

    /// Returns the exact request identity admitted for the pending save.
    #[must_use]
    pub fn pending_save_request_id(&self) -> Option<&artisan_domain::RequestId> {
        self.pending_save
            .as_ref()
            .map(|pending| &pending.request_id)
    }

    /// Returns the operation represented by the visible redacted failure.
    #[must_use]
    pub fn failure_operation(&self) -> Option<EngineSettingsFailureOperation> {
        if self.input_error.is_some() {
            Some(EngineSettingsFailureOperation::Input)
        } else if self.save_failure.is_some() {
            Some(EngineSettingsFailureOperation::Save)
        } else if self.settings_failure.is_some() {
            Some(EngineSettingsFailureOperation::SettingsRead)
        } else if self.registry_failure.is_some() {
            Some(EngineSettingsFailureOperation::Registry)
        } else {
            None
        }
    }

    /// Returns the finite service failure, if one is visible.
    #[must_use]
    pub fn service_failure(&self) -> Option<ServiceFailure> {
        self.save_failure
            .or(self.settings_failure)
            .or(self.registry_failure)
    }

    /// Returns the finite input error, if one is visible.
    #[must_use]
    pub const fn input_error(&self) -> Option<EngineConfigError> {
        self.input_error
    }

    /// Returns the current visible status.
    #[must_use]
    pub fn status(&self) -> EngineSettingsStatus {
        if self.selected_thread.is_none() {
            return EngineSettingsStatus::Unselected;
        }
        if self.settings_load.conflict_refreshing {
            return EngineSettingsStatus::ConflictRefreshing;
        }
        if self.pending_save.is_some() {
            return EngineSettingsStatus::Saving;
        }
        if self.failure_operation().is_some() {
            return EngineSettingsStatus::Failure;
        }
        match self.registry_view() {
            RegistryView::Missing => return EngineSettingsStatus::RegistryMissing,
            RegistryView::PresentEmpty => return EngineSettingsStatus::RegistryPresentEmpty,
            _ => {}
        }
        if self.loading_thread.is_some() || self.registry_load.loading || self.settings_load.needed
        {
            return EngineSettingsStatus::Loading;
        }
        if self.is_dirty_internal() {
            return EngineSettingsStatus::Dirty;
        }
        match &self.settings {
            Some(ThreadEngineSettingsResult::Unconfigured { .. }) => {
                EngineSettingsStatus::Unconfigured
            }
            Some(ThreadEngineSettingsResult::Configured { .. }) => EngineSettingsStatus::Ready,
            None => EngineSettingsStatus::Loading,
        }
    }

    /// Returns whether Save is enabled for the current draft.
    #[must_use]
    pub fn can_save(&self) -> bool {
        if !self.is_editable() || !self.is_dirty_internal() || self.input_error.is_some() {
            return false;
        }
        let Some(RegisteredEngineProfilesResult::RegistryPresent { .. }) = self.registry.as_ref()
        else {
            return false;
        };
        self.draft.build_config(self.registry.as_ref()).is_ok()
    }

    /// Returns whether Cancel may discard the local draft.
    #[must_use]
    pub fn can_cancel(&self) -> bool {
        self.selected_thread.is_some()
            && self.is_dirty_internal()
            && self.pending_save.is_none()
            && !self.settings_load.conflict_refreshing
            && self.loading_thread.is_none()
    }

    /// Returns the authoritative revision when configured.
    #[must_use]
    pub fn revision(&self) -> Option<EngineConfigRevision> {
        self.authoritative_revision
    }

    /// Returns the authoritative config when configured.
    #[must_use]
    pub fn authoritative_config(&self) -> Option<&EngineRunConfig> {
        self.authoritative_config.as_ref()
    }

    /// Returns whether a registry load is needed (cached for lifetime).
    #[must_use]
    pub fn needs_registry_load(&self) -> bool {
        self.registry_load.needed
    }

    /// Returns whether an authoritative settings load still needs admission.
    #[must_use]
    pub const fn needs_settings_load(&self) -> bool {
        self.settings_load.needed
    }

    /// Returns whether a conflict refresh is waiting for command admission.
    #[must_use]
    pub fn pending_reload_thread(&self) -> Option<&ThreadId> {
        self.needs_settings_reload.as_ref()
    }

    /// Marks the catalogue command admitted by the bounded bridge.
    pub fn mark_registry_load_admitted(&mut self) {
        self.registry_load.needed = false;
        self.registry_load.loading = true;
        self.registry_failure = None;
    }

    /// Retains the catalogue need after a Busy/Stopped admission refusal.
    pub fn on_registry_load_admission_failed(&mut self, failure: ServiceFailure) {
        self.registry_load.loading = false;
        self.registry_load.needed = self.selected_thread.is_some();
        self.registry_failure = Some(failure);
    }

    /// Mints the next application-owned settings-load generation.
    ///
    /// # Errors
    ///
    /// Returns a redacted request failure when no thread is selected, a load
    /// is already active, or the monotonic generation is exhausted.
    pub fn prepare_settings_load(&mut self) -> Result<SettingsLoadGeneration, ServiceFailure> {
        if self.selected_thread.is_none() || self.active_settings_generation.is_some() {
            return Err(ServiceFailure {
                stage: ServiceFailureStage::Request,
                category: ServiceFailureCategory::InvalidConfiguration,
            });
        }
        let generation = match self.next_settings_load_generation {
            None => SettingsLoadGeneration::first(),
            Some(previous) => match previous.checked_next() {
                Some(next) => next,
                None => {
                    return Err(ServiceFailure {
                        stage: ServiceFailureStage::Request,
                        category: ServiceFailureCategory::Integrity,
                    });
                }
            },
        };
        self.next_settings_load_generation = Some(generation);
        Ok(generation)
    }

    /// Commits settings-load state only after its command was admitted.
    #[must_use]
    pub fn mark_settings_load_admitted(
        &mut self,
        thread_id: &ThreadId,
        generation: SettingsLoadGeneration,
    ) -> bool {
        if self.selected_thread.as_ref() != Some(thread_id)
            || self.active_settings_generation.is_some()
            || self.next_settings_load_generation != Some(generation)
            || (self.needs_settings_reload.as_ref() != Some(thread_id)
                && !self.settings_load.needed)
        {
            return false;
        }
        self.active_settings_generation = Some(generation);
        self.loading_thread = Some(thread_id.clone());
        self.settings_load.needed = false;
        if self.needs_settings_reload.as_ref() == Some(thread_id) {
            self.needs_settings_reload = None;
        }
        self.settings_failure = None;
        true
    }

    /// Re-arms a settings read after its command was refused by the bridge.
    pub fn on_settings_load_admission_failed(
        &mut self,
        thread_id: ThreadId,
        failure: ServiceFailure,
    ) {
        if self.selected_thread.as_ref() != Some(&thread_id) {
            return;
        }
        self.active_settings_generation = None;
        self.loading_thread = None;
        if failure.category == ServiceFailureCategory::Integrity {
            // Generation exhaustion is terminal: no later command may reuse
            // an identity or pretend that an authoritative refresh is safe.
            self.settings_load.needed = false;
            self.needs_settings_reload = None;
        } else if self.settings_load.conflict_refreshing {
            self.needs_settings_reload = Some(thread_id);
        } else {
            self.settings_load.needed = true;
        }
        self.settings_failure = Some(failure);
    }

    /// Selects a real thread, clearing prior draft and stale state.
    pub fn select_thread(&mut self, thread_id: Option<&ThreadId>) {
        if self.selected_thread.as_ref() == thread_id {
            return;
        }
        self.selected_thread = thread_id.cloned();
        self.settings = None;
        self.authoritative_revision = None;
        self.authoritative_config = None;
        self.loading_thread = None;
        self.active_settings_generation = None;
        self.settings_load.needed = thread_id.is_some();
        self.draft = EngineSettingsDraft::default();
        self.pending_save = None;
        self.settings_load.conflict_refreshing = false;
        self.settings_failure = None;
        self.save_failure = None;
        self.input_error = None;
        self.needs_settings_reload = None;
        if self.registry.is_none() && !self.registry_load.loading && thread_id.is_some() {
            self.registry_load.needed = true;
        } else if self.registry.is_some() {
            self.registry_load.needed = false;
        }
    }

    /// Handles a successful registry result, preserving it for the application lifetime.
    pub fn on_registry_loaded(&mut self, result: RegisteredEngineProfilesResult) {
        self.registry = Some(result);
        self.registry_load.loading = false;
        self.registry_load.needed = false;
        self.registry_failure = None;
    }

    /// Handles a registry failure as redacted.
    pub fn on_registry_failed(&mut self, failure: ServiceFailure) {
        self.registry_load.loading = false;
        self.registry_load.needed = self.selected_thread.is_some();
        self.registry_failure = Some(failure);
    }

    /// Handles an authoritative settings result only for its exact active
    /// thread and application-minted generation.
    pub fn on_settings_loaded(
        &mut self,
        generation: SettingsLoadGeneration,
        result: ThreadEngineSettingsResult,
    ) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        if self.active_settings_generation != Some(generation)
            || self.loading_thread.as_ref() != Some(&selected)
            || result.thread_id() != &selected
        {
            return;
        }
        self.active_settings_generation = None;
        self.loading_thread = None;
        self.settings_load.needed = false;
        if self.settings_load.conflict_refreshing {
            self.settings_load.conflict_refreshing = false;
            self.needs_settings_reload = None;
        }
        self.settings_failure = None;
        self.input_error = None;
        match &result {
            ThreadEngineSettingsResult::Unconfigured { .. } => {
                self.authoritative_revision = None;
                self.authoritative_config = None;
                self.draft = EngineSettingsDraft::default();
            }
            ThreadEngineSettingsResult::Configured {
                revision, config, ..
            } => {
                self.authoritative_revision = Some(*revision);
                self.authoritative_config = Some((**config).clone());
                self.draft = EngineSettingsDraft::from_config(config);
            }
        }
        self.settings = Some(result);
    }

    /// Marks a save in flight only after the exact command was admitted.
    #[must_use]
    pub fn begin_saving(
        &mut self,
        pending_thread: ThreadId,
        request_id: artisan_domain::RequestId,
        retained: EngineRunConfig,
    ) -> bool {
        if self.selected_thread.as_ref() != Some(&pending_thread)
            || self.pending_save.is_some()
            || !self.can_save()
        {
            return false;
        }
        self.pending_save = Some(PendingSave {
            thread_id: pending_thread,
            request_id,
            retained,
        });
        self.save_failure = None;
        self.input_error = None;
        true
    }

    /// Records a Busy/Stopped save admission refusal without losing the draft.
    pub fn on_save_admission_failed(&mut self, failure: ServiceFailure) {
        self.pending_save = None;
        if self.selected_thread.is_some() {
            self.save_failure = Some(failure);
        }
    }

    /// Handles a successful save, storing returned revision plus retained config.
    pub fn on_save_succeeded(
        &mut self,
        result: &SetThreadEngineConfigResult,
        retained: EngineRunConfig,
    ) {
        let Some(selected) = self.selected_thread.as_ref() else {
            return;
        };
        let Some(pending) = self.pending_save.as_ref() else {
            return;
        };
        if result.thread_id.as_str() != selected.as_str()
            || result.thread_id.as_str() != pending.thread_id.as_str()
            || result.request_id.as_str() != pending.request_id.as_str()
            || pending.retained != retained
        {
            return;
        }
        self.authoritative_revision = Some(result.revision);
        self.authoritative_config = Some(retained.clone());
        self.draft = EngineSettingsDraft::from_config(&retained);
        self.settings = Some(ThreadEngineSettingsResult::Configured {
            thread_id: selected.clone(),
            revision: result.revision,
            config: Box::new(retained),
        });
        self.pending_save = None;
        self.save_failure = None;
        self.input_error = None;
        self.settings_load.conflict_refreshing = false;
        self.needs_settings_reload = None;
    }

    /// Handles a conflict event only for the exact pending save request.
    pub fn on_conflict(&mut self, thread_id: ThreadId, request_id: &artisan_domain::RequestId) {
        let Some(selected) = self.selected_thread.as_ref() else {
            return;
        };
        if thread_id.as_str() != selected.as_str() {
            return;
        }
        if self.pending_save.as_ref().is_none_or(|pending| {
            pending.thread_id.as_str() != thread_id.as_str()
                || pending.request_id.as_str() != request_id.as_str()
        }) {
            return;
        }
        self.pending_save = None;
        self.active_settings_generation = None;
        self.loading_thread = None;
        self.settings_load.conflict_refreshing = true;
        self.needs_settings_reload = Some(thread_id);
        self.save_failure = None;
    }

    /// Handles a redacted save failure for the exact pending request.
    pub fn on_save_failed(
        &mut self,
        thread_id: &ThreadId,
        request_id: &artisan_domain::RequestId,
        failure: ServiceFailure,
    ) {
        let Some(selected) = self.selected_thread.as_ref() else {
            return;
        };
        if thread_id.as_str() != selected.as_str() {
            return;
        }
        if self.pending_save.as_ref().is_none_or(|pending| {
            pending.thread_id.as_str() != thread_id.as_str()
                || pending.request_id.as_str() != request_id.as_str()
        }) {
            return;
        }
        self.pending_save = None;
        self.save_failure = Some(failure);
    }

    /// Handles a settings-read failure for the exact active generation.
    pub fn on_settings_load_failed(
        &mut self,
        thread_id: ThreadId,
        generation: SettingsLoadGeneration,
        failure: ServiceFailure,
    ) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        if thread_id != selected
            || self.active_settings_generation != Some(generation)
            || self.loading_thread.as_ref() != Some(&thread_id)
        {
            return;
        }
        self.active_settings_generation = None;
        self.loading_thread = None;
        if self.settings_load.conflict_refreshing {
            self.needs_settings_reload = Some(thread_id);
        } else {
            self.settings_load.needed = false;
        }
        self.settings_failure = Some(failure);
    }

    /// Cancels local edits without emitting a save.
    pub fn cancel(&mut self) {
        if !self.can_cancel() {
            return;
        }
        if let Some(config) = self.authoritative_config.clone() {
            self.draft = EngineSettingsDraft::from_config(&config);
        } else {
            self.draft = EngineSettingsDraft::default();
        }
        self.input_error = None;
        self.save_failure = None;
    }

    fn is_dirty_internal(&self) -> bool {
        let base = if let Some(config) = &self.authoritative_config {
            EngineSettingsDraft::from_config(config)
        } else {
            EngineSettingsDraft::default()
        };
        self.draft != base
    }

    /// Returns whether draft differs from authoritative.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.is_dirty_internal()
    }

    fn is_editable(&self) -> bool {
        self.selected_thread.is_some()
            && self.settings.is_some()
            && self.loading_thread.is_none()
            && self.active_settings_generation.is_none()
            && self.pending_save.is_none()
            && !self.settings_load.conflict_refreshing
    }

    /// Selects one profile only when it came from the authoritative registry.
    #[must_use]
    pub fn select_profile(&mut self, profile_id: &EngineProfileId) -> bool {
        let present = matches!(
            self.registry.as_ref(),
            Some(RegisteredEngineProfilesResult::RegistryPresent { profile_ids })
                if profile_ids.iter().any(|id| id == profile_id)
        );
        if !present || !self.is_editable() {
            self.input_error = Some(EngineConfigError::new(
                "profile_id",
                EngineConfigReason::InvalidIdentifier,
            ));
            return false;
        }
        profile_id.as_str().clone_into(&mut self.draft.profile_id);
        self.input_error = None;
        true
    }

    /// Applies a complete, strictly structured clipboard configuration.
    ///
    /// # Errors
    ///
    /// Returns only a redacted field/reason pair; the source document is
    /// never stored or formatted.
    pub fn apply_manual_configuration(&mut self, document: &str) -> Result<(), EngineConfigError> {
        if !self.is_editable() {
            let error = EngineConfigError::new("configuration", EngineConfigReason::Unsupported);
            self.input_error = Some(error);
            return Err(error);
        }
        let parsed = match parse_manual_configuration(document) {
            Ok(draft) => draft,
            Err(error) => {
                self.input_error = Some(error);
                return Err(error);
            }
        };
        self.draft = parsed;
        self.input_error = None;
        self.save_failure = None;
        Ok(())
    }

    /// Builds the pending `SetThreadEngineConfig` when Save is enabled.
    ///
    /// Caller must supply a fresh `RequestId` minted for this save. The
    /// transport will retain it across one reconnect retry.
    #[must_use]
    pub fn build_save_command(
        &self,
        request_id: artisan_domain::RequestId,
    ) -> Option<artisan_domain::SetThreadEngineConfig> {
        if !self.can_save() {
            return None;
        }
        let thread_id = self.selected_thread.clone()?;
        let config = self.draft.build_config(self.registry.as_ref()).ok()?;
        let precondition = self.authoritative_revision.map_or_else(
            || artisan_domain::EngineConfigUpdatePrecondition::Unconfigured,
            artisan_domain::EngineConfigUpdatePrecondition::Exact,
        );
        Some(artisan_domain::SetThreadEngineConfig::new(
            request_id,
            thread_id,
            precondition,
            config,
        ))
    }
}

impl Default for EngineSettingsController {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{
        EngineConfigRevision, EngineProfileId, EngineRunConfig, RequestId, ThreadId,
    };
    use artisan_protocol::{
        RegisteredEngineProfilesResult, SetThreadEngineConfigResult, ThreadEngineSettingsResult,
    };

    fn thread_id(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("thread id")
    }

    fn profile_id(value: &str) -> EngineProfileId {
        EngineProfileId::parse(value).expect("profile id")
    }

    fn request_id(value: &str) -> RequestId {
        RequestId::parse(value).expect("request id")
    }

    fn revision(value: u64) -> EngineConfigRevision {
        EngineConfigRevision::new(value).expect("revision")
    }

    fn sample_draft(profile: &str) -> EngineSettingsDraft {
        EngineSettingsDraft {
            profile_id: profile.to_owned(),
            model_id: "model-test".to_owned(),
            route_id: "route-test".to_owned(),
            variant_id: String::new(),
            permission_id: "permission-test".to_owned(),
            agent_id: "agent-test".to_owned(),
            approval: "never".to_owned(),
            filesystem: "none".to_owned(),
            network: "disabled".to_owned(),
            web_search: "disabled".to_owned(),
            attempt_budget: "5".to_owned(),
            readiness_budget: "1".to_owned(),
            health_budget: "1".to_owned(),
            prompt_budget: "1".to_owned(),
            stream_budget: "1".to_owned(),
            close_budget: "1".to_owned(),
            max_json_body_bytes: "1".to_owned(),
            max_sse_line_bytes: "1".to_owned(),
            max_sse_event_bytes: "1".to_owned(),
            max_readiness_line_bytes: "1".to_owned(),
            max_header_count: "1".to_owned(),
            max_http_buffer_bytes: "1".to_owned(),
            max_stderr_bytes: "1".to_owned(),
            observation_capacity: "1".to_owned(),
        }
    }

    fn sample_config(profile: &str) -> EngineRunConfig {
        let draft = sample_draft(profile);
        let registry = RegisteredEngineProfilesResult::RegistryPresent {
            profile_ids: vec![profile_id(profile)],
        };
        draft.build_config(Some(&registry)).expect("sample config")
    }

    fn registered_present(ids: &[&str]) -> RegisteredEngineProfilesResult {
        RegisteredEngineProfilesResult::RegistryPresent {
            profile_ids: ids.iter().map(|id| profile_id(id)).collect(),
        }
    }

    fn admit_settings_load(
        controller: &mut EngineSettingsController,
        thread_id: &ThreadId,
    ) -> SettingsLoadGeneration {
        let generation = controller.prepare_settings_load().expect("generation");
        assert!(controller.mark_settings_load_admitted(thread_id, generation));
        generation
    }

    fn load_unconfigured(controller: &mut EngineSettingsController, thread_id: &ThreadId) {
        let generation = admit_settings_load(controller, thread_id);
        controller.on_settings_loaded(
            generation,
            ThreadEngineSettingsResult::Unconfigured {
                thread_id: thread_id.clone(),
            },
        );
    }

    fn load_configured(
        controller: &mut EngineSettingsController,
        thread_id: &ThreadId,
        revision_value: u64,
        config: EngineRunConfig,
    ) {
        let generation = admit_settings_load(controller, thread_id);
        controller.on_settings_loaded(
            generation,
            ThreadEngineSettingsResult::Configured {
                thread_id: thread_id.clone(),
                revision: revision(revision_value),
                config: Box::new(config),
            },
        );
    }

    fn ready_controller() -> (EngineSettingsController, ThreadId, EngineRunConfig) {
        let mut controller = EngineSettingsController::new();
        let thread = thread_id("thread-a");
        controller.select_thread(Some(&thread));
        controller.on_registry_loaded(registered_present(&["default"]));
        let config = sample_config("default");
        load_configured(&mut controller, &thread, 7, config.clone());
        (controller, thread, config)
    }

    fn make_dirty_config(controller: &mut EngineSettingsController) -> EngineRunConfig {
        controller.draft_mut().model_id = "model-next".to_owned();
        controller
            .draft()
            .build_config(Some(&registered_present(&["default"])))
            .expect("dirty config")
    }

    fn manual_document(draft: &EngineSettingsDraft) -> String {
        let mut document = String::new();
        for key in MANUAL_CONFIGURATION_KEYS {
            let value = match key {
                "profile_id" => &draft.profile_id,
                "model_id" => &draft.model_id,
                "route_id" => &draft.route_id,
                "variant_id" => &draft.variant_id,
                "permission_id" => &draft.permission_id,
                "agent_id" => &draft.agent_id,
                "approval" => &draft.approval,
                "filesystem" => &draft.filesystem,
                "network" => &draft.network,
                "web_search" => &draft.web_search,
                "attempt_budget" => &draft.attempt_budget,
                "readiness_budget" => &draft.readiness_budget,
                "health_budget" => &draft.health_budget,
                "prompt_budget" => &draft.prompt_budget,
                "stream_budget" => &draft.stream_budget,
                "close_budget" => &draft.close_budget,
                "max_json_body_bytes" => &draft.max_json_body_bytes,
                "max_sse_line_bytes" => &draft.max_sse_line_bytes,
                "max_sse_event_bytes" => &draft.max_sse_event_bytes,
                "max_readiness_line_bytes" => &draft.max_readiness_line_bytes,
                "max_header_count" => &draft.max_header_count,
                "max_http_buffer_bytes" => &draft.max_http_buffer_bytes,
                "max_stderr_bytes" => &draft.max_stderr_bytes,
                "observation_capacity" => &draft.observation_capacity,
                _ => unreachable!("manual field table diverged"),
            };
            document.push_str(key);
            document.push('=');
            document.push_str(value);
            document.push('\n');
        }
        document
    }

    fn bridge_busy() -> ServiceFailure {
        ServiceFailure {
            stage: ServiceFailureStage::EventBridge,
            category: ServiceFailureCategory::Backpressure,
        }
    }

    fn bridge_stopped() -> ServiceFailure {
        ServiceFailure {
            stage: ServiceFailureStage::EventBridge,
            category: ServiceFailureCategory::ChannelClosed,
        }
    }

    #[test]
    fn configured_load_becomes_ready_with_authoritative_config() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default", "work"]));
        let config = sample_config("default");
        load_configured(&mut controller, &tid, 7, config.clone());
        assert_eq!(controller.status(), EngineSettingsStatus::Ready);
        assert_eq!(controller.revision(), Some(revision(7)));
        assert_eq!(controller.authoritative_config(), Some(&config));
        assert!(!controller.is_dirty());
        assert!(!controller.can_save());
        controller.draft_mut().model_id = "model-next".to_owned();
        assert!(controller.can_save());
    }

    #[test]
    fn unconfigured_load_becomes_unconfigured_and_requires_explicit_values() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
        assert_eq!(controller.revision(), None);
        assert!(!controller.can_save());
        assert_eq!(controller.draft().profile_id, "");
        assert_eq!(controller.draft().model_id, "");
    }

    #[test]
    fn registry_missing_present_empty_and_exact_ids_are_distinct() {
        let mut controller = EngineSettingsController::new();
        controller.select_thread(Some(&thread_id("thread-a")));
        controller.on_registry_loaded(RegisteredEngineProfilesResult::RegistryMissing);
        assert_eq!(controller.registry_view(), RegistryView::Missing);
        assert_eq!(controller.status(), EngineSettingsStatus::RegistryMissing);

        controller.on_registry_loaded(RegisteredEngineProfilesResult::RegistryPresent {
            profile_ids: Vec::new(),
        });
        assert_eq!(controller.registry_view(), RegistryView::PresentEmpty);
        assert_eq!(
            controller.status(),
            EngineSettingsStatus::RegistryPresentEmpty
        );

        controller.on_registry_loaded(registered_present(&["alpha", "beta"]));
        assert!(matches!(controller.registry_view(), RegistryView::Present(ids) if ids.len()==2));
        load_unconfigured(&mut controller, &thread_id("thread-a"));
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
        // Valid profile must be one of returned ids.
        controller.draft_mut().profile_id = "gamma".to_owned();
        assert!(
            controller
                .draft
                .build_config(controller.registry.as_ref())
                .is_err()
        );
        controller.draft_mut().profile_id = "alpha".to_owned();
        // Still needs other fields, but profile validation now passes.
        assert!(
            controller
                .draft
                .build_config(controller.registry.as_ref())
                .is_err()
        );
    }

    #[test]
    fn edits_are_local_dirty_and_cancel_does_not_emit_save() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        controller.draft_mut().profile_id = "default".to_owned();
        controller.draft_mut().model_id = "model-test".to_owned();
        // Not yet complete, still dirty but cannot save.
        assert!(controller.is_dirty());
        assert_eq!(controller.status(), EngineSettingsStatus::Dirty);
        assert!(!controller.can_save());
        controller.cancel();
        assert!(!controller.is_dirty());
        assert_eq!(controller.draft().profile_id, "");
    }

    #[test]
    fn complete_validation_required_and_no_defaults_appear() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        assert_eq!(controller.draft().profile_id, "");
        assert!(!controller.can_save());
        // Fill every required field explicitly.
        let valid = sample_config("default");
        controller
            .draft_mut()
            .clone_from(&EngineSettingsDraft::from_config(&valid));
        assert!(controller.can_save());
        assert!(
            controller
                .build_save_command(request_id("request-a"))
                .is_some()
        );
        // Empty variant is explicit None, not synthesized.
        assert_eq!(controller.draft().variant_id, "");
    }

    #[test]
    fn save_success_applies_returned_revision_plus_retained_config() {
        let (mut controller, tid, _) = ready_controller();
        let config = make_dirty_config(&mut controller);
        let request = request_id("request-a");
        assert!(controller.begin_saving(tid.clone(), request.clone(), config.clone()));
        assert_eq!(controller.status(), EngineSettingsStatus::Saving);
        let result = SetThreadEngineConfigResult {
            request_id: request,
            thread_id: tid.clone(),
            revision: revision(1),
            disposition: artisan_domain::ReceiptDisposition::Accepted,
        };
        controller.on_save_succeeded(&result, config.clone());
        assert_eq!(controller.status(), EngineSettingsStatus::Ready);
        assert_eq!(controller.revision(), Some(revision(1)));
        assert_eq!(controller.authoritative_config(), Some(&config));
        assert!(!controller.is_dirty());
    }

    #[test]
    fn conflict_emits_exactly_one_authoritative_refresh() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        let config = sample_config("default");
        load_configured(&mut controller, &tid, 3, config);
        let new_config = make_dirty_config(&mut controller);
        let request = request_id("request-a");
        assert!(controller.begin_saving(tid.clone(), request.clone(), new_config));
        controller.on_conflict(tid.clone(), &request);
        assert_eq!(
            controller.status(),
            EngineSettingsStatus::ConflictRefreshing
        );
        assert_eq!(controller.pending_reload_thread(), Some(&tid));
        let generation = admit_settings_load(&mut controller, &tid);
        assert!(controller.pending_reload_thread().is_none());
        controller.on_settings_loaded(
            generation,
            ThreadEngineSettingsResult::Unconfigured {
                thread_id: tid.clone(),
            },
        );
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
        // Second conflict without pending should be ignored.
        controller.on_conflict(tid, &request_id("request-a"));
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
    }

    #[test]
    fn stale_load_responses_are_fenced_by_thread_and_generation() {
        let mut controller = EngineSettingsController::new();
        let first = thread_id("thread-a");
        let second = thread_id("thread-b");
        controller.select_thread(Some(&first));
        controller.on_registry_loaded(registered_present(&["default"]));
        let first_generation = admit_settings_load(&mut controller, &first);
        controller.select_thread(Some(&second));
        let second_generation = admit_settings_load(&mut controller, &second);
        controller.select_thread(Some(&first));
        let current_generation = admit_settings_load(&mut controller, &first);
        let stale_first = ThreadEngineSettingsResult::Configured {
            thread_id: first.clone(),
            revision: revision(5),
            config: Box::new(sample_config("default")),
        };
        controller.on_settings_loaded(first_generation, stale_first);
        controller.on_settings_load_failed(first.clone(), first_generation, bridge_busy());
        controller.on_settings_load_failed(second.clone(), second_generation, bridge_busy());
        controller.on_settings_loaded(
            second_generation,
            ThreadEngineSettingsResult::Unconfigured { thread_id: second },
        );
        assert!(controller.authoritative_settings().is_none());
        controller.on_settings_loaded(
            current_generation,
            ThreadEngineSettingsResult::Unconfigured { thread_id: first },
        );
        assert!(matches!(
            controller.authoritative_settings(),
            Some(ThreadEngineSettingsResult::Unconfigured { .. })
        ));
    }

    #[test]
    fn stale_save_responses_are_fenced_by_exact_request_identity() {
        let (mut controller, tid, _) = ready_controller();
        let retained = make_dirty_config(&mut controller);
        let request = request_id("request-a");
        assert!(controller.begin_saving(tid.clone(), request.clone(), retained.clone()));
        let stale_request = request_id("request-b");
        controller.on_save_failed(&tid, &stale_request, bridge_busy());
        controller.on_conflict(tid.clone(), &stale_request);
        controller.on_save_succeeded(
            &SetThreadEngineConfigResult {
                request_id: stale_request,
                thread_id: tid.clone(),
                revision: revision(9),
                disposition: artisan_domain::ReceiptDisposition::Accepted,
            },
            retained.clone(),
        );
        assert_eq!(controller.status(), EngineSettingsStatus::Saving);
        assert_eq!(controller.pending_save_request_id(), Some(&request));
        controller.on_save_succeeded(
            &SetThreadEngineConfigResult {
                request_id: request,
                thread_id: tid,
                revision: revision(9),
                disposition: artisan_domain::ReceiptDisposition::Accepted,
            },
            retained,
        );
        assert_eq!(controller.status(), EngineSettingsStatus::Ready);
    }

    #[test]
    fn cancel_is_noop_while_an_admitted_save_is_pending() {
        let (mut controller, tid, _) = ready_controller();
        let retained = make_dirty_config(&mut controller);
        let draft_before = controller.draft().clone();
        assert!(controller.begin_saving(tid, request_id("request-a"), retained,));
        assert!(!controller.can_cancel());
        controller.cancel();
        assert_eq!(controller.draft(), &draft_before);
        assert_eq!(controller.status(), EngineSettingsStatus::Saving);
    }

    #[test]
    fn bridge_admission_refusals_rearm_without_losing_draft_or_claiming_progress() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_load_admission_failed(bridge_busy());
        assert!(controller.needs_registry_load());
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::Registry)
        );

        controller.on_registry_loaded(registered_present(&["default"]));
        controller.on_settings_load_admission_failed(tid.clone(), bridge_stopped());
        assert!(controller.needs_settings_load());
        assert!(controller.active_settings_generation().is_none());
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::SettingsRead)
        );

        load_unconfigured(&mut controller, &tid);
        let retained = sample_config("default");
        controller
            .draft_mut()
            .clone_from(&EngineSettingsDraft::from_config(&retained));
        controller.draft_mut().model_id = "model-next".to_owned();
        let draft_before = controller.draft().clone();
        controller.on_save_admission_failed(bridge_stopped());
        assert!(controller.pending_save_request_id().is_none());
        assert_eq!(controller.draft(), &draft_before);
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::Save)
        );
    }

    #[test]
    fn save_admission_failure_clears_defensive_pending_state() {
        let (mut controller, tid, _) = ready_controller();
        let retained = make_dirty_config(&mut controller);
        let draft_before = controller.draft().clone();
        assert!(controller.begin_saving(tid, request_id("request-a"), retained));
        controller.on_save_admission_failed(bridge_busy());
        assert!(controller.pending_save_request_id().is_none());
        assert_eq!(controller.draft(), &draft_before);
        assert_eq!(
            controller.failure_operation(),
            Some(EngineSettingsFailureOperation::Save)
        );
    }

    #[test]
    fn generation_exhaustion_fails_closed_without_rearming_the_read() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.next_settings_load_generation =
            Some(SettingsLoadGeneration::from_raw_for_test(u64::MAX));
        let failure = controller.prepare_settings_load().expect_err("exhausted");
        assert_eq!(failure.category, ServiceFailureCategory::Integrity);
        controller.on_settings_load_admission_failed(tid, failure);
        assert!(!controller.needs_settings_load());
        assert!(controller.active_settings_generation().is_none());
    }

    #[test]
    fn certified_profile_selection_is_checked_against_the_authoritative_registry() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        assert!(!controller.select_profile(&profile_id("unregistered")));
        assert_eq!(controller.draft().profile_id, "");
        assert!(controller.select_profile(&profile_id("default")));
        assert_eq!(controller.draft().profile_id, "default");
    }

    #[test]
    fn manual_configuration_is_complete_bounded_and_does_not_retain_rejected_text() {
        let template = manual_configuration_template();
        assert_eq!(template.lines().count(), MANUAL_CONFIGURATION_KEYS.len());
        assert_eq!(
            parse_manual_configuration(&template),
            Ok(EngineSettingsDraft::default())
        );

        let expected = sample_draft("default");
        let document = manual_document(&expected);
        assert_eq!(parse_manual_configuration(&document), Ok(expected.clone()));

        let unknown = template.replacen("profile_id=", "unknown=", 1);
        let unknown_error = parse_manual_configuration(&unknown).expect_err("unknown key");
        assert_eq!(unknown_error.field(), "configuration");
        assert!(parse_manual_configuration("profile_id=\nprofile_id=\n").is_err());
        assert!(parse_manual_configuration("profile_id=has=extra\n").is_err());
        let missing = template
            .lines()
            .take(MANUAL_CONFIGURATION_KEYS.len() - 1)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(parse_manual_configuration(&missing).is_err());
        assert!(
            parse_manual_configuration(&"x".repeat(MAX_MANUAL_CONFIGURATION_BYTES + 1)).is_err()
        );

        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(&tid));
        controller.on_registry_loaded(registered_present(&["default"]));
        load_unconfigured(&mut controller, &tid);
        let rejected = "profile_id=secret-profile\n";
        assert!(controller.apply_manual_configuration(rejected).is_err());
        assert!(!format!("{:?}", controller.input_error()).contains("secret-profile"));
        assert_eq!(controller.draft(), &EngineSettingsDraft::default());
        assert!(!controller.can_save());
    }

    #[test]
    fn empty_variant_is_the_only_explicit_absence() {
        let mut draft = sample_draft("default");
        assert!(
            draft
                .build_config(Some(&registered_present(&["default"])))
                .is_ok()
        );
        draft.variant_id = " ".to_owned();
        assert!(
            draft
                .build_config(Some(&registered_present(&["default"])))
                .is_err()
        );
    }

    #[test]
    fn failure_is_redacted_and_does_not_reveal_source_values() {
        let failure = ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::Peer,
        };
        let display = failure.to_string();
        assert!(!display.contains("model-test"));
        assert!(!display.contains("route-test"));
        assert!(!display.contains("default"));
    }

    #[test]
    fn registry_is_cached_for_application_lifetime() {
        let mut controller = EngineSettingsController::new();
        controller.select_thread(Some(&thread_id("thread-a")));
        controller.on_registry_loaded(registered_present(&["default"]));
        assert!(!controller.needs_registry_load());
        controller.select_thread(Some(&thread_id("thread-b")));
        assert!(!controller.needs_registry_load());
        assert_eq!(
            controller.registry_view(),
            RegistryView::Present(vec![profile_id("default")])
        );
    }
}
