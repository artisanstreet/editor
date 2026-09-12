use std::num::NonZeroU64;

use crate::bounds::{
    ENGINE_RUNTIME_MAX_HEADER_COUNT, ENGINE_RUNTIME_MAX_LINE_BYTES,
    ENGINE_RUNTIME_MAX_SSE_EVENT_BYTES, ENGINE_RUNTIME_MAX_STDERR_BYTES,
};
use crate::identifiers::{EngineAgentId, PermissionId};

use super::identity::{
    ByteLimit, CountLimit, EngineConfigError, EngineConfigReason, EngineId, EngineSelection,
    FiniteMillis,
};
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

/// Caller-supplied bounded budgets and transport capacities for one engine
/// attempt.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EngineRuntimeControlsInput {
    /// Complete budget for one engine attempt.
    pub attempt_budget: FiniteMillis,
    /// Budget for engine readiness.
    pub readiness_budget: FiniteMillis,
    /// Budget for engine health checking.
    pub health_budget: FiniteMillis,
    /// Budget for sending the prompt.
    pub prompt_budget: FiniteMillis,
    /// Budget for receiving the engine stream.
    pub stream_budget: FiniteMillis,
    /// Budget for closing the engine connection.
    pub close_budget: FiniteMillis,
    /// Maximum JSON request or response body size.
    pub max_json_body_bytes: ByteLimit,
    /// Maximum size of one SSE line.
    pub max_sse_line_bytes: ByteLimit,
    /// Maximum size of one SSE event.
    pub max_sse_event_bytes: ByteLimit,
    /// Maximum size of one readiness response line.
    pub max_readiness_line_bytes: ByteLimit,
    /// Maximum number of HTTP headers.
    pub max_header_count: CountLimit,
    /// Maximum HTTP buffer size.
    pub max_http_buffer_bytes: ByteLimit,
    /// Maximum captured stderr size.
    pub max_stderr_bytes: ByteLimit,
    /// Maximum retained observation count.
    pub observation_capacity: CountLimit,
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
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when phase budgets overflow or exceed
    /// the attempt budget, buffers violate their containment relationships, or
    /// a transport limit exceeds its documented bound.
    pub fn new(input: EngineRuntimeControlsInput) -> Result<Self, EngineConfigError> {
        let EngineRuntimeControlsInput {
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
        } = input;
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

    /// Returns the selected engine implementation.
    #[must_use]
    pub const fn engine_id(&self) -> EngineId {
        self.selection.engine_id()
    }

    /// Returns the storage codec version for this configuration.
    ///
    /// Version 1 preserves the byte-identical legacy `OpenCode` 2 encoding.
    /// Version 2 carries the tagged per-engine shape. Newly representable
    /// engines always encode as version 2.
    #[must_use]
    pub const fn storage_codec_version(&self) -> u16 {
        match self.selection.engine_id() {
            EngineId::OpenCode2 => 1,
            EngineId::Codex | EngineId::Claude | EngineId::Grok | EngineId::Cursor => 2,
        }
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
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when `value` is zero or exceeds
    /// `i64::MAX`, the largest value representable by the SQLite column.
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
        self.get().cast_signed()
    }

    /// Advances a revision without crossing the SQLite signed boundary.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when the next revision would exceed
    /// `i64::MAX`, the largest value representable by the SQLite column.
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
