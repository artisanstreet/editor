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
    ServiceFailure, ServiceFailureCategory, ServiceFailureStage,
};

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
#[derive(Clone, Debug, Eq, PartialEq)]
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

impl Default for EngineSettingsDraft {
    fn default() -> Self {
        Self {
            profile_id: String::new(),
            model_id: String::new(),
            route_id: String::new(),
            variant_id: String::new(),
            permission_id: String::new(),
            agent_id: String::new(),
            approval: String::new(),
            filesystem: String::new(),
            network: String::new(),
            web_search: String::new(),
            attempt_budget: String::new(),
            readiness_budget: String::new(),
            health_budget: String::new(),
            prompt_budget: String::new(),
            stream_budget: String::new(),
            close_budget: String::new(),
            max_json_body_bytes: String::new(),
            max_sse_line_bytes: String::new(),
            max_sse_event_bytes: String::new(),
            max_readiness_line_bytes: String::new(),
            max_header_count: String::new(),
            max_http_buffer_bytes: String::new(),
            max_stderr_bytes: String::new(),
            observation_capacity: String::new(),
        }
    }
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
        let profile_id = EngineProfileId::parse(self.profile_id.clone()).map_err(|_| {
            EngineConfigError::new("profile_id", EngineConfigReason::InvalidIdentifier)
        })?;
        if let Some(RegisteredEngineProfilesResult::RegistryPresent { profile_ids }) = registry {
            if !profile_ids
                .iter()
                .any(|id| id.as_str() == profile_id.as_str())
            {
                return Err(EngineConfigError::new(
                    "profile_id",
                    EngineConfigReason::InvalidIdentifier,
                ));
            }
        } else if matches!(
            registry,
            Some(RegisteredEngineProfilesResult::RegistryMissing)
        ) {
            return Err(EngineConfigError::new(
                "profile_id",
                EngineConfigReason::InvalidIdentifier,
            ));
        }
        let model_id = EngineModelId::parse(self.model_id.clone()).map_err(|_| {
            EngineConfigError::new("model_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let route_id = EngineRouteId::parse(self.route_id.clone()).map_err(|_| {
            EngineConfigError::new("route_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let variant_id = if self.variant_id.trim().is_empty() {
            None
        } else {
            Some(
                EngineVariantId::parse(self.variant_id.clone()).map_err(|_| {
                    EngineConfigError::new("variant_id", EngineConfigReason::InvalidIdentifier)
                })?,
            )
        };
        let permission_id = PermissionId::parse(self.permission_id.clone()).map_err(|_| {
            EngineConfigError::new("permission_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let agent_id = EngineAgentId::parse(self.agent_id.clone()).map_err(|_| {
            EngineConfigError::new("agent_id", EngineConfigReason::InvalidIdentifier)
        })?;
        let approval = match self.approval.as_str() {
            "never" => ApprovalMode::Never,
            "on_request" => ApprovalMode::OnRequest,
            "always" => ApprovalMode::Always,
            _ => {
                return Err(EngineConfigError::new(
                    "approval",
                    EngineConfigReason::Unsupported,
                ));
            }
        };
        let filesystem = match self.filesystem.as_str() {
            "none" => FilesystemAccess::None,
            "workspace" => FilesystemAccess::Workspace,
            "host" => FilesystemAccess::Host,
            _ => {
                return Err(EngineConfigError::new(
                    "filesystem",
                    EngineConfigReason::Unsupported,
                ));
            }
        };
        let network = match self.network.as_str() {
            "disabled" => NetworkAccess::Disabled,
            "enabled" => NetworkAccess::Enabled,
            _ => {
                return Err(EngineConfigError::new(
                    "network",
                    EngineConfigReason::Unsupported,
                ));
            }
        };
        let web_search = match self.web_search.as_str() {
            "disabled" => WebSearchAccess::Disabled,
            "enabled" => WebSearchAccess::Enabled,
            _ => {
                return Err(EngineConfigError::new(
                    "web_search",
                    EngineConfigReason::Unsupported,
                ));
            }
        };
        let permission = EnginePermissionPolicy::new(
            permission_id,
            agent_id,
            approval,
            filesystem,
            network,
            web_search,
        );
        let selection = EngineSelection::OpenCode2(OpenCode2Selection::new(
            profile_id, model_id, route_id, variant_id, permission,
        ));
        let parse_millis = |field: &'static str, value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| EngineConfigError::new(field, EngineConfigReason::InvalidIdentifier))
                .and_then(|v| {
                    FiniteMillis::new(v).map_err(|e| EngineConfigError::new(field, e.reason()))
                })
        };
        let parse_bytes = |field: &'static str, value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| EngineConfigError::new(field, EngineConfigReason::InvalidIdentifier))
                .and_then(|v| {
                    ByteLimit::new(v).map_err(|e| EngineConfigError::new(field, e.reason()))
                })
        };
        let parse_count = |field: &'static str, value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| EngineConfigError::new(field, EngineConfigReason::InvalidIdentifier))
                .and_then(|v| {
                    CountLimit::new(v).map_err(|e| EngineConfigError::new(field, e.reason()))
                })
        };
        let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
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
        })?;
        Ok(EngineRunConfig::new(selection, runtime))
    }
}

/// Application-owned controller for thread-bound engine settings.
pub struct EngineSettingsController {
    selected_thread: Option<ThreadId>,
    registry: Option<RegisteredEngineProfilesResult>,
    registry_loading: bool,
    settings: Option<ThreadEngineSettingsResult>,
    loading_thread: Option<ThreadId>,
    draft: EngineSettingsDraft,
    authoritative_revision: Option<EngineConfigRevision>,
    authoritative_config: Option<EngineRunConfig>,
    is_saving: bool,
    pending_thread: Option<ThreadId>,
    pending_retained: Option<EngineRunConfig>,
    conflict_refreshing: bool,
    failure: Option<ServiceFailure>,
    needs_registry_load: bool,
    needs_settings_reload: Option<ThreadId>,
}

impl EngineSettingsController {
    /// Creates an empty controller with no selection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            selected_thread: None,
            registry: None,
            registry_loading: false,
            settings: None,
            loading_thread: None,
            draft: EngineSettingsDraft::default(),
            authoritative_revision: None,
            authoritative_config: None,
            is_saving: false,
            pending_thread: None,
            pending_retained: None,
            conflict_refreshing: false,
            failure: None,
            needs_registry_load: false,
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
        if self.registry_loading {
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

    /// Returns the current visible status.
    #[must_use]
    pub fn status(&self) -> EngineSettingsStatus {
        if self.selected_thread.is_none() {
            return EngineSettingsStatus::Unselected;
        }
        if self.failure.is_some()
            && !self.conflict_refreshing
            && !self.is_saving
            && self.loading_thread.is_none()
        {
            return EngineSettingsStatus::Failure;
        }
        if self.conflict_refreshing {
            return EngineSettingsStatus::ConflictRefreshing;
        }
        if self.is_saving {
            return EngineSettingsStatus::Saving;
        }
        if self.loading_thread.is_some() || self.registry_loading {
            return EngineSettingsStatus::Loading;
        }
        match self.registry_view() {
            RegistryView::Missing => return EngineSettingsStatus::RegistryMissing,
            RegistryView::PresentEmpty => {
                // Still need to distinguish unconfigured/ready but UI shows present-empty.
                // Surface as present-empty when no failure and not loading.
                // For status we keep underlying readiness but expose view separately.
                // To satisfy required distinct observation, return this variant when registry empty.
                return EngineSettingsStatus::RegistryPresentEmpty;
            }
            _ => {}
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
        if self.selected_thread.is_none()
            || self.is_saving
            || self.conflict_refreshing
            || self.loading_thread.is_some()
        {
            return false;
        }
        if self.failure.is_some() {
            return false;
        }
        self.draft.build_config(self.registry.as_ref()).is_ok()
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
        self.needs_registry_load
    }

    /// Clears the one-shot registry load flag after the caller submits it.
    pub fn clear_registry_load_flag(&mut self) {
        self.needs_registry_load = false;
        self.registry_loading = true;
    }

    /// Returns a pending authoritative reload when conflict occurred.
    #[must_use]
    pub fn take_reload_thread(&mut self) -> Option<ThreadId> {
        self.needs_settings_reload.take()
    }

    /// Selects a real thread, clearing prior draft and stale state.
    pub fn select_thread(&mut self, thread_id: Option<ThreadId>) {
        if self.selected_thread == thread_id {
            return;
        }
        self.selected_thread.clone_from(&thread_id);
        self.settings = None;
        self.authoritative_revision = None;
        self.authoritative_config = None;
        self.loading_thread = thread_id.clone();
        self.draft = EngineSettingsDraft::default();
        self.is_saving = false;
        self.pending_thread = None;
        self.pending_retained = None;
        self.conflict_refreshing = false;
        self.failure = None;
        self.needs_settings_reload = None;
        if self.registry.is_none() && thread_id.is_some() {
            self.needs_registry_load = true;
            self.registry_loading = false;
        } else {
            self.needs_registry_load = false;
        }
    }

    /// Handles a successful registry result, preserving it for the application lifetime.
    pub fn on_registry_loaded(&mut self, result: RegisteredEngineProfilesResult) {
        // Registry is global, not per-thread; keep first successful result unless missing vs present distinction needs update.
        // But spec allows caching; we preserve exact returned value.
        self.registry = Some(result);
        self.registry_loading = false;
        self.needs_registry_load = false;
    }

    /// Handles a registry failure as redacted.
    pub fn on_registry_failed(&mut self, failure: ServiceFailure) {
        self.registry_loading = false;
        self.failure = Some(failure);
    }

    /// Handles an authoritative settings result, discarding stale thread responses.
    pub fn on_settings_loaded(&mut self, result: ThreadEngineSettingsResult) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        if result.thread_id() != &selected {
            return;
        }
        if self.loading_thread.as_ref() != Some(&selected) && !self.conflict_refreshing {
            // Late response after thread switch; still discard if thread mismatch already handled.
            // Allow loading_thread None when reload after conflict: then accept if selected matches.
            if self.loading_thread.is_none() && !self.conflict_refreshing {
                // Stale after switch cleared loading; still accept if thread equals selected but loading cleared due to switch? Actually switch clears loading to new thread; old thread would mismatch and already returned.
            }
        }
        if self.conflict_refreshing {
            self.conflict_refreshing = false;
            self.needs_settings_reload = None;
        }
        self.loading_thread = None;
        self.failure = None;
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
        self.is_saving = false;
        self.pending_thread = None;
        self.pending_retained = None;
    }

    /// Marks that a save is in flight, retaining the exact immutable config.
    pub fn begin_saving(&mut self, pending_thread: ThreadId, retained: EngineRunConfig) {
        self.is_saving = true;
        self.pending_thread = Some(pending_thread);
        self.pending_retained = Some(retained);
        self.failure = None;
    }

    /// Handles a successful save, storing returned revision plus retained config.
    pub fn on_save_succeeded(
        &mut self,
        result: SetThreadEngineConfigResult,
        retained: EngineRunConfig,
    ) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        let Some(pending) = self.pending_thread.clone() else {
            return;
        };
        if &result.thread_id != &selected || &result.thread_id != &pending {
            return;
        }
        if self.pending_retained.as_ref() != Some(&retained) {
            // Stale save with different retained config; ignore.
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
        self.is_saving = false;
        self.pending_thread = None;
        self.pending_retained = None;
        self.failure = None;
        self.conflict_refreshing = false;
    }

    /// Handles a conflict event, discarding pending save and requiring one reload.
    pub fn on_conflict(&mut self, thread_id: ThreadId) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        if thread_id != selected {
            return;
        }
        if self.pending_thread.as_ref() != Some(&thread_id) {
            return;
        }
        self.is_saving = false;
        self.pending_thread = None;
        self.pending_retained = None;
        self.conflict_refreshing = true;
        self.loading_thread = Some(thread_id.clone());
        self.needs_settings_reload = Some(thread_id);
    }

    /// Handles a redacted save failure.
    pub fn on_save_failed(&mut self, thread_id: ThreadId, failure: ServiceFailure) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        if thread_id != selected {
            return;
        }
        if self.pending_thread.as_ref() != Some(&thread_id) {
            return;
        }
        self.is_saving = false;
        self.pending_thread = None;
        self.pending_retained = None;
        self.failure = Some(failure);
    }

    /// Handles a generic load failure.
    pub fn on_load_failed(&mut self, thread_id: ThreadId, failure: ServiceFailure) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        if thread_id != selected {
            return;
        }
        if self.loading_thread.as_ref() != Some(&thread_id) {
            return;
        }
        self.loading_thread = None;
        self.failure = Some(failure);
    }

    /// Cancels local edits without emitting a save.
    pub fn cancel(&mut self) {
        if let Some(config) = self.authoritative_config.clone() {
            self.draft = EngineSettingsDraft::from_config(&config);
        } else {
            self.draft = EngineSettingsDraft::default();
        }
        self.is_saving = false;
        self.pending_thread = None;
        self.pending_retained = None;
        // Keep failure/conflict as is; cancel does not clear failure.
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

    fn sample_config(profile: &str) -> EngineRunConfig {
        let draft = EngineSettingsDraft {
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
        };
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

    #[test]
    fn configured_load_becomes_ready_with_authoritative_config() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(tid.clone()));
        controller.on_registry_loaded(registered_present(&["default", "work"]));
        let config = sample_config("default");
        let result = ThreadEngineSettingsResult::Configured {
            thread_id: tid.clone(),
            revision: revision(7),
            config: Box::new(config.clone()),
        };
        controller.on_settings_loaded(result);
        assert_eq!(controller.status(), EngineSettingsStatus::Ready);
        assert_eq!(controller.revision(), Some(revision(7)));
        assert_eq!(controller.authoritative_config(), Some(&config));
        assert!(!controller.is_dirty());
        assert!(controller.can_save());
    }

    #[test]
    fn unconfigured_load_becomes_unconfigured_and_requires_explicit_values() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(tid.clone()));
        controller.on_registry_loaded(registered_present(&["default"]));
        controller.on_settings_loaded(ThreadEngineSettingsResult::Unconfigured {
            thread_id: tid.clone(),
        });
        assert_eq!(controller.status(), EngineSettingsStatus::Unconfigured);
        assert_eq!(controller.revision(), None);
        assert!(!controller.can_save());
        assert_eq!(controller.draft().profile_id, "");
        assert_eq!(controller.draft().model_id, "");
    }

    #[test]
    fn registry_missing_present_empty_and_exact_ids_are_distinct() {
        let mut controller = EngineSettingsController::new();
        controller.select_thread(Some(thread_id("thread-a")));
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
        controller.select_thread(Some(tid.clone()));
        controller.on_registry_loaded(registered_present(&["default"]));
        controller.on_settings_loaded(ThreadEngineSettingsResult::Unconfigured {
            thread_id: tid.clone(),
        });
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
        controller.select_thread(Some(tid.clone()));
        controller.on_registry_loaded(registered_present(&["default"]));
        controller.on_settings_loaded(ThreadEngineSettingsResult::Unconfigured {
            thread_id: tid.clone(),
        });
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
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(tid.clone()));
        controller.on_registry_loaded(registered_present(&["default"]));
        controller.on_settings_loaded(ThreadEngineSettingsResult::Unconfigured {
            thread_id: tid.clone(),
        });
        let config = sample_config("default");
        controller
            .draft_mut()
            .clone_from(&EngineSettingsDraft::from_config(&config));
        controller.begin_saving(tid.clone(), config.clone());
        assert_eq!(controller.status(), EngineSettingsStatus::Saving);
        let result = SetThreadEngineConfigResult {
            request_id: request_id("request-a"),
            thread_id: tid.clone(),
            revision: revision(1),
            disposition: artisan_domain::ReceiptDisposition::Accepted,
        };
        controller.on_save_succeeded(result, config.clone());
        assert_eq!(controller.status(), EngineSettingsStatus::Ready);
        assert_eq!(controller.revision(), Some(revision(1)));
        assert_eq!(controller.authoritative_config(), Some(&config));
        assert!(!controller.is_dirty());
    }

    #[test]
    fn conflict_emits_exactly_one_authoritative_refresh() {
        let mut controller = EngineSettingsController::new();
        let tid = thread_id("thread-a");
        controller.select_thread(Some(tid.clone()));
        controller.on_registry_loaded(registered_present(&["default"]));
        let config = sample_config("default");
        controller.on_settings_loaded(ThreadEngineSettingsResult::Configured {
            thread_id: tid.clone(),
            revision: revision(3),
            config: Box::new(config.clone()),
        });
        let new_config = sample_config("default");
        controller
            .draft_mut()
            .clone_from(&EngineSettingsDraft::from_config(&new_config));
        controller.begin_saving(tid.clone(), new_config.clone());
        controller.on_conflict(tid.clone());
        assert_eq!(
            controller.status(),
            EngineSettingsStatus::ConflictRefreshing
        );
        assert_eq!(controller.take_reload_thread(), Some(tid.clone()));
        assert!(controller.take_reload_thread().is_none());
        // Second conflict without pending should be ignored.
        controller.on_conflict(tid.clone());
        assert_eq!(
            controller.status(),
            EngineSettingsStatus::ConflictRefreshing
        );
    }

    #[test]
    fn stale_load_and_save_responses_are_ignored_after_thread_switch() {
        let mut controller = EngineSettingsController::new();
        let first = thread_id("thread-a");
        let second = thread_id("thread-b");
        controller.select_thread(Some(first.clone()));
        controller.on_registry_loaded(registered_present(&["default"]));
        // Switch before first load arrives.
        controller.select_thread(Some(second.clone()));
        let config = sample_config("default");
        let stale = ThreadEngineSettingsResult::Configured {
            thread_id: first.clone(),
            revision: revision(5),
            config: Box::new(config.clone()),
        };
        controller.on_settings_loaded(stale);
        assert_eq!(controller.selected_thread(), Some(&second));
        assert_eq!(controller.settings, None);
        // Stale save should also be discarded.
        controller.begin_saving(second.clone(), config.clone());
        let stale_result = SetThreadEngineConfigResult {
            request_id: request_id("request-a"),
            thread_id: first.clone(),
            revision: revision(9),
            disposition: artisan_domain::ReceiptDisposition::Accepted,
        };
        controller.on_save_succeeded(stale_result, config.clone());
        assert_eq!(controller.revision(), None);
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
        controller.select_thread(Some(thread_id("thread-a")));
        controller.on_registry_loaded(registered_present(&["default"]));
        assert!(!controller.needs_registry_load());
        controller.select_thread(Some(thread_id("thread-b")));
        assert!(!controller.needs_registry_load());
        assert_eq!(
            controller.registry_view(),
            RegistryView::Present(vec![profile_id("default")])
        );
    }
}
