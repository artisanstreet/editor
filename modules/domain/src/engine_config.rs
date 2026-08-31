//! Validated, durable configuration for the native engine run path.
//!
//! The configuration types deliberately keep their representations private.
//! Callers must construct a complete value through the checked constructors,
//! which gives the database and protocol layers one place to enforce the
//! runtime bounds without carrying secrets or host paths in diagnostics.

use std::num::NonZeroU64;

use thiserror::Error;

use crate::bounds::{
    ENGINE_RUNTIME_MAX_BODY_BYTES, ENGINE_RUNTIME_MAX_HEADER_COUNT, ENGINE_RUNTIME_MAX_LINE_BYTES,
    ENGINE_RUNTIME_MAX_MILLIS, ENGINE_RUNTIME_MAX_OBSERVATIONS, ENGINE_RUNTIME_MAX_SSE_EVENT_BYTES,
    ENGINE_RUNTIME_MAX_STDERR_BYTES,
};
use crate::identifiers::{
    EngineAgentId, EngineModelId, EngineProfileId, EngineRouteId, EngineVariantId, PermissionId,
};

/// Bounded category for a rejected engine configuration field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineConfigReason {
    /// The value was outside its documented finite range.
    OutOfRange,
    /// Several otherwise valid values violated a relationship.
    Inconsistent,
    /// The field did not contain a valid domain identifier.
    InvalidIdentifier,
    /// The field used a value that this engine version does not implement.
    Unsupported,
}

impl std::fmt::Display for EngineConfigReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::OutOfRange => "out of range",
            Self::Inconsistent => "inconsistent",
            Self::InvalidIdentifier => "invalid identifier",
            Self::Unsupported => "unsupported",
        })
    }
}

/// Safe, bounded validation failure for an engine configuration.
///
/// Only a stable field label and a finite reason category are retained. The
/// rejected value is intentionally never stored or formatted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
#[error("invalid engine configuration field {field}: {reason}")]
pub struct EngineConfigError {
    field: &'static str,
    reason: EngineConfigReason,
}

impl EngineConfigError {
    /// Creates a bounded configuration error for one field.
    #[must_use]
    pub const fn new(field: &'static str, reason: EngineConfigReason) -> Self {
        Self { field, reason }
    }

    /// Returns the stable field label.
    #[must_use]
    pub const fn field(self) -> &'static str {
        self.field
    }

    /// Returns the bounded reason category.
    #[must_use]
    pub const fn reason(self) -> EngineConfigReason {
        self.reason
    }
}

/// A positive, bounded duration in milliseconds.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FiniteMillis(u64);

impl FiniteMillis {
    /// Creates a duration in the inclusive `1..=86_400_000` range.
    pub const fn new(value: u64) -> Result<Self, EngineConfigError> {
        if value == 0 || value > ENGINE_RUNTIME_MAX_MILLIS {
            Err(EngineConfigError::new(
                "runtime duration",
                EngineConfigReason::OutOfRange,
            ))
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the duration in milliseconds.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A positive byte limit accepted by an engine transport boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ByteLimit(u64);

impl ByteLimit {
    /// Creates a limit in the inclusive `1..=8_388_608` range.
    pub const fn new(value: u64) -> Result<Self, EngineConfigError> {
        if value == 0 || value > ENGINE_RUNTIME_MAX_BODY_BYTES {
            Err(EngineConfigError::new(
                "byte limit",
                EngineConfigReason::OutOfRange,
            ))
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the byte limit.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A positive count limit accepted by an engine transport boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CountLimit(u64);

impl CountLimit {
    /// Creates a limit in the inclusive `1..=4_096` range.
    pub const fn new(value: u64) -> Result<Self, EngineConfigError> {
        if value == 0 || value > ENGINE_RUNTIME_MAX_OBSERVATIONS {
            Err(EngineConfigError::new(
                "count limit",
                EngineConfigReason::OutOfRange,
            ))
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the count limit.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Engine implementation selected by a thread configuration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EngineId {
    /// The first supported native engine implementation.
    OpenCode2,
}

impl EngineId {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenCode2 => "opencode2",
        }
    }
}

/// Engine-specific selection values.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EngineSelection {
    /// OpenCode 2 selection.
    OpenCode2(OpenCode2Selection),
}

impl EngineSelection {
    /// Returns the selected engine implementation.
    #[must_use]
    pub const fn engine_id(&self) -> EngineId {
        match self {
            Self::OpenCode2(_) => EngineId::OpenCode2,
        }
    }

    /// Borrows the OpenCode 2 selection.
    #[must_use]
    pub const fn as_opencode2(&self) -> &OpenCode2Selection {
        match self {
            Self::OpenCode2(selection) => selection,
        }
    }
}

/// Complete selection for the OpenCode 2 engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct OpenCode2Selection {
    profile_id: EngineProfileId,
    model_id: EngineModelId,
    route_id: EngineRouteId,
    variant_id: Option<EngineVariantId>,
    permission: EnginePermissionPolicy,
}

impl OpenCode2Selection {
    /// Constructs a complete engine selection, including an explicit variant
    /// absence when no variant is selected.
    #[must_use]
    pub fn new(
        profile_id: EngineProfileId,
        model_id: EngineModelId,
        route_id: EngineRouteId,
        variant_id: Option<EngineVariantId>,
        permission: EnginePermissionPolicy,
    ) -> Self {
        Self {
            profile_id,
            model_id,
            route_id,
            variant_id,
            permission,
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the model identity.
    #[must_use]
    pub const fn model_id(&self) -> &EngineModelId {
        &self.model_id
    }

    /// Returns the route identity.
    #[must_use]
    pub const fn route_id(&self) -> &EngineRouteId {
        &self.route_id
    }

    /// Returns the optional variant identity.
    #[must_use]
    pub const fn variant_id(&self) -> Option<&EngineVariantId> {
        self.variant_id.as_ref()
    }

    /// Returns the permission policy.
    #[must_use]
    pub const fn permission(&self) -> &EnginePermissionPolicy {
        &self.permission
    }
}

/// Explicit permission policy attached to one engine selection.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EnginePermissionPolicy {
    permission_id: PermissionId,
    agent_id: EngineAgentId,
    approval: ApprovalMode,
    filesystem: FilesystemAccess,
    network: NetworkAccess,
    web_search: WebSearchAccess,
}

impl EnginePermissionPolicy {
    /// Constructs a complete permission policy.
    #[must_use]
    pub fn new(
        permission_id: PermissionId,
        agent_id: EngineAgentId,
        approval: ApprovalMode,
        filesystem: FilesystemAccess,
        network: NetworkAccess,
        web_search: WebSearchAccess,
    ) -> Self {
        Self {
            permission_id,
            agent_id,
            approval,
            filesystem,
            network,
            web_search,
        }
    }

    /// Returns the permission identity.
    #[must_use]
    pub const fn permission_id(&self) -> &PermissionId {
        &self.permission_id
    }

    /// Returns the agent identity.
    #[must_use]
    pub const fn agent_id(&self) -> &EngineAgentId {
        &self.agent_id
    }

    /// Returns the approval mode.
    #[must_use]
    pub const fn approval(&self) -> ApprovalMode {
        self.approval
    }

    /// Returns the filesystem access level.
    #[must_use]
    pub const fn filesystem(&self) -> FilesystemAccess {
        self.filesystem
    }

    /// Returns the network access level.
    #[must_use]
    pub const fn network(&self) -> NetworkAccess {
        self.network
    }

    /// Returns the web-search access level.
    #[must_use]
    pub const fn web_search(&self) -> WebSearchAccess {
        self.web_search
    }
}

/// Approval policy for engine actions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApprovalMode {
    /// Never ask for interactive approval.
    Never,
    /// Ask only when the engine requests approval.
    OnRequest,
    /// Always require approval.
    Always,
}

impl ApprovalMode {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::OnRequest => "on_request",
            Self::Always => "always",
        }
    }
}

/// Filesystem scope granted to the engine.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FilesystemAccess {
    /// No filesystem access.
    None,
    /// Access limited to the active workspace.
    Workspace,
    /// Host filesystem access.
    Host,
}

impl FilesystemAccess {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Workspace => "workspace",
            Self::Host => "host",
        }
    }
}

/// General network access policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NetworkAccess {
    /// Network access is disabled.
    Disabled,
    /// Network access is enabled.
    Enabled,
}

impl NetworkAccess {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
        }
    }
}

/// Web-search-specific network policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WebSearchAccess {
    /// Web search is disabled.
    Disabled,
    /// Web search is enabled.
    Enabled,
}

impl WebSearchAccess {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Enabled => "enabled",
        }
    }
}

/// Bounded budgets and transport capacities for one engine attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EngineRuntimeControls {
    attempt_budget: FiniteMillis,
    readiness_budget: FiniteMillis,
    health_budget: FiniteMillis,
    prompt_budget: FiniteMillis,
    stream_budget: FiniteMillis,
    close_budget: FiniteMillis,
    max_json_body_bytes: ByteLimit,
    max_sse_line_bytes: ByteLimit,
    max_sse_event_bytes: ByteLimit,
    max_readiness_line_bytes: ByteLimit,
    max_header_count: CountLimit,
    max_http_buffer_bytes: ByteLimit,
    max_stderr_bytes: ByteLimit,
    observation_capacity: CountLimit,
}

impl EngineRuntimeControls {
    /// Constructs complete runtime controls after checking phase and buffer
    /// relationships.
    pub fn new(
        attempt_budget: FiniteMillis,
        readiness_budget: FiniteMillis,
        health_budget: FiniteMillis,
        prompt_budget: FiniteMillis,
        stream_budget: FiniteMillis,
        close_budget: FiniteMillis,
        max_json_body_bytes: ByteLimit,
        max_sse_line_bytes: ByteLimit,
        max_sse_event_bytes: ByteLimit,
        max_readiness_line_bytes: ByteLimit,
        max_header_count: CountLimit,
        max_http_buffer_bytes: ByteLimit,
        max_stderr_bytes: ByteLimit,
        observation_capacity: CountLimit,
    ) -> Result<Self, EngineConfigError> {
        let phase_sum = readiness_budget
            .get()
            .checked_add(health_budget.get())
            .and_then(|sum| sum.checked_add(prompt_budget.get()))
            .and_then(|sum| sum.checked_add(stream_budget.get()))
            .and_then(|sum| sum.checked_add(close_budget.get()))
            .ok_or_else(|| {
                EngineConfigError::new("phase budgets", EngineConfigReason::Inconsistent)
            })?;
        if phase_sum > attempt_budget.get() {
            return Err(EngineConfigError::new(
                "phase budgets",
                EngineConfigReason::Inconsistent,
            ));
        }
        if max_sse_line_bytes.get() > max_sse_event_bytes.get() {
            return Err(EngineConfigError::new(
                "max_sse_line_bytes",
                EngineConfigReason::Inconsistent,
            ));
        }
        if max_readiness_line_bytes.get() > max_http_buffer_bytes.get() {
            return Err(EngineConfigError::new(
                "max_readiness_line_bytes",
                EngineConfigReason::Inconsistent,
            ));
        }
        if max_sse_event_bytes.get() > ENGINE_RUNTIME_MAX_SSE_EVENT_BYTES {
            return Err(EngineConfigError::new(
                "max_sse_event_bytes",
                EngineConfigReason::OutOfRange,
            ));
        }
        if max_sse_line_bytes.get() > ENGINE_RUNTIME_MAX_LINE_BYTES
            || max_readiness_line_bytes.get() > ENGINE_RUNTIME_MAX_LINE_BYTES
        {
            return Err(EngineConfigError::new(
                "runtime line bytes",
                EngineConfigReason::OutOfRange,
            ));
        }
        if max_header_count.get() > ENGINE_RUNTIME_MAX_HEADER_COUNT {
            return Err(EngineConfigError::new(
                "max_header_count",
                EngineConfigReason::OutOfRange,
            ));
        }
        if max_stderr_bytes.get() > ENGINE_RUNTIME_MAX_STDERR_BYTES {
            return Err(EngineConfigError::new(
                "max_stderr_bytes",
                EngineConfigReason::OutOfRange,
            ));
        }

        Ok(Self {
            attempt_budget,
            readiness_budget,
            health_budget,
            prompt_budget,
            stream_budget,
            close_budget,
            max_json_body_bytes,
            max_sse_line_bytes,
            max_sse_event_bytes,
            max_readiness_line_bytes,
            max_header_count,
            max_http_buffer_bytes,
            max_stderr_bytes,
            observation_capacity,
        })
    }

    /// Returns the complete attempt budget.
    #[must_use]
    pub const fn attempt_budget(self) -> FiniteMillis {
        self.attempt_budget
    }

    /// Returns the readiness budget.
    #[must_use]
    pub const fn readiness_budget(self) -> FiniteMillis {
        self.readiness_budget
    }

    /// Returns the health budget.
    #[must_use]
    pub const fn health_budget(self) -> FiniteMillis {
        self.health_budget
    }

    /// Returns the prompt budget.
    #[must_use]
    pub const fn prompt_budget(self) -> FiniteMillis {
        self.prompt_budget
    }

    /// Returns the stream budget.
    #[must_use]
    pub const fn stream_budget(self) -> FiniteMillis {
        self.stream_budget
    }

    /// Returns the close budget.
    #[must_use]
    pub const fn close_budget(self) -> FiniteMillis {
        self.close_budget
    }

    /// Returns the JSON body limit.
    #[must_use]
    pub const fn max_json_body_bytes(self) -> ByteLimit {
        self.max_json_body_bytes
    }

    /// Returns the SSE line limit.
    #[must_use]
    pub const fn max_sse_line_bytes(self) -> ByteLimit {
        self.max_sse_line_bytes
    }

    /// Returns the SSE event limit.
    #[must_use]
    pub const fn max_sse_event_bytes(self) -> ByteLimit {
        self.max_sse_event_bytes
    }

    /// Returns the readiness line limit.
    #[must_use]
    pub const fn max_readiness_line_bytes(self) -> ByteLimit {
        self.max_readiness_line_bytes
    }

    /// Returns the header count limit.
    #[must_use]
    pub const fn max_header_count(self) -> CountLimit {
        self.max_header_count
    }

    /// Returns the HTTP buffer limit.
    #[must_use]
    pub const fn max_http_buffer_bytes(self) -> ByteLimit {
        self.max_http_buffer_bytes
    }

    /// Returns the stderr limit.
    #[must_use]
    pub const fn max_stderr_bytes(self) -> ByteLimit {
        self.max_stderr_bytes
    }

    /// Returns the observation capacity.
    #[must_use]
    pub const fn observation_capacity(self) -> CountLimit {
        self.observation_capacity
    }
}

/// Complete immutable configuration captured by one engine run.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineRunConfig {
    selection: EngineSelection,
    runtime: EngineRuntimeControls,
}

impl EngineRunConfig {
    /// Constructs a complete immutable run configuration.
    #[must_use]
    pub fn new(selection: EngineSelection, runtime: EngineRuntimeControls) -> Self {
        Self { selection, runtime }
    }

    /// Returns the engine selection.
    #[must_use]
    pub const fn selection(&self) -> &EngineSelection {
        &self.selection
    }

    /// Returns the runtime controls.
    #[must_use]
    pub const fn runtime(&self) -> EngineRuntimeControls {
        self.runtime
    }
}

/// One-based optimistic-concurrency revision for a configured thread.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EngineConfigRevision(NonZeroU64);

impl EngineConfigRevision {
    /// Creates a revision in the inclusive `1..=i64::MAX` range.
    pub const fn new(value: u64) -> Result<Self, EngineConfigError> {
        if value == 0 || value > i64::MAX as u64 {
            return Err(EngineConfigError::new(
                "revision",
                EngineConfigReason::OutOfRange,
            ));
        }
        match NonZeroU64::new(value) {
            Some(value) => Ok(Self(value)),
            None => Err(EngineConfigError::new(
                "revision",
                EngineConfigReason::OutOfRange,
            )),
        }
    }

    /// Returns the revision as a positive unsigned integer.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    /// Returns the revision in the SQLite signed integer representation.
    #[must_use]
    pub const fn as_i64(self) -> i64 {
        self.get() as i64
    }

    /// Advances a revision without crossing the SQLite signed boundary.
    pub const fn checked_next(self) -> Result<Self, EngineConfigError> {
        match self.get().checked_add(1) {
            Some(next) => Self::new(next),
            None => Err(EngineConfigError::new(
                "revision",
                EngineConfigReason::OutOfRange,
            )),
        }
    }
}

/// Optimistic precondition for changing a thread's engine configuration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EngineConfigUpdatePrecondition {
    /// The thread must still be in its unconfigured sentinel state.
    Unconfigured,
    /// The thread must have exactly this current revision.
    Exact(EngineConfigRevision),
}

impl EngineConfigUpdatePrecondition {
    /// Returns the expected configured revision, if any.
    #[must_use]
    pub const fn expected_revision(self) -> Option<EngineConfigRevision> {
        match self {
            Self::Unconfigured => None,
            Self::Exact(revision) => Some(revision),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(attempt: u64) -> Result<EngineRuntimeControls, EngineConfigError> {
        let one = FiniteMillis::new(1)?;
        EngineRuntimeControls::new(
            FiniteMillis::new(attempt)?,
            one,
            one,
            one,
            one,
            one,
            ByteLimit::new(1)?,
            ByteLimit::new(1)?,
            ByteLimit::new(1)?,
            ByteLimit::new(1)?,
            CountLimit::new(1)?,
            ByteLimit::new(1)?,
            ByteLimit::new(1)?,
            CountLimit::new(1)?,
        )
    }

    fn complete_config(profile: &str) -> EngineRunConfig {
        let permission = EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::Never,
            FilesystemAccess::None,
            NetworkAccess::Disabled,
            WebSearchAccess::Disabled,
        );
        EngineRunConfig::new(
            EngineSelection::OpenCode2(OpenCode2Selection::new(
                EngineProfileId::parse(profile).expect("profile id is valid"),
                EngineModelId::parse("model-test").expect("model id is valid"),
                EngineRouteId::parse("route-test").expect("route id is valid"),
                None,
                permission,
            )),
            runtime(5).expect("runtime is valid"),
        )
    }

    #[test]
    fn bounds_are_checked_at_the_private_value_boundaries() {
        assert!(FiniteMillis::new(0).is_err());
        assert!(FiniteMillis::new(86_400_001).is_err());
        assert!(ByteLimit::new(0).is_err());
        assert!(ByteLimit::new(8_388_609).is_err());
        assert!(CountLimit::new(0).is_err());
        assert!(CountLimit::new(4_097).is_err());
        assert!(EngineConfigRevision::new(0).is_err());
        assert!(EngineConfigRevision::new(i64::MAX as u64 + 1).is_err());
        assert!(EngineConfigRevision::new(1).is_ok());
    }

    #[test]
    fn phase_and_containing_buffer_relationships_are_checked() {
        let one = FiniteMillis::new(1).expect("one millisecond is valid");
        let bytes = |value| ByteLimit::new(value).expect("byte limit is valid");
        let count = |value| CountLimit::new(value).expect("count limit is valid");
        assert!(
            EngineRuntimeControls::new(
                FiniteMillis::new(4).expect("attempt budget is valid"),
                one,
                one,
                one,
                one,
                one,
                bytes(1),
                bytes(2),
                bytes(1),
                bytes(1),
                count(1),
                bytes(1),
                bytes(1),
                count(1),
            )
            .is_err()
        );
        assert!(
            EngineRuntimeControls::new(
                FiniteMillis::new(5).expect("attempt budget is valid"),
                one,
                one,
                one,
                one,
                one,
                bytes(1),
                bytes(2),
                bytes(1),
                bytes(1),
                count(1),
                bytes(1),
                bytes(1),
                count(1),
            )
            .is_err()
        );
    }

    #[test]
    fn missing_variant_is_explicit_and_default_is_an_ordinary_profile_id() {
        let config = complete_config("default");
        let selection = config.selection().as_opencode2();
        assert_eq!(selection.profile_id().as_str(), "default");
        assert!(selection.variant_id().is_none());
    }
}
