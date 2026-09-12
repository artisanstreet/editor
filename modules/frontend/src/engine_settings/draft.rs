//! Manual configuration document parsing and validated draft building.
//!
//! Extracted verbatim from `engine_settings.rs` during the module split.

#[allow(clippy::wildcard_imports)]
use super::*;

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
    ///
    /// The manual settings surface stays `OpenCode` 2-shaped in this packet.
    /// Any other engine contributes its shared profile/model identity and
    /// its canonical policy.
    #[must_use]
    pub fn from_config(config: &EngineRunConfig) -> Self {
        let selection = config.selection();
        let runtime = config.runtime();
        let (model_id, route_id, variant_id) = match selection {
            EngineSelection::OpenCode2(selection) => (
                selection.model_id().as_str().to_owned(),
                selection.route_id().as_str().to_owned(),
                selection
                    .variant_id()
                    .map_or_else(String::new, |id| id.as_str().to_owned()),
            ),
            other => (
                other
                    .model_id()
                    .map_or_else(String::new, |id| id.as_str().to_owned()),
                String::new(),
                String::new(),
            ),
        };
        let permission = selection.permission();
        Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model_id,
            route_id,
            variant_id,
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
