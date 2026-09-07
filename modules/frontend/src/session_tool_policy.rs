//! Pure policy for the built-in tool permissions of one session.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/live-workspace/session-tool-policy.ts`. The
//! TypeScript policy reads only `permission_mode` and `sandbox_mode` from a
//! durable `ThreadSessionPolicy`, then produces the seven fields consumed by
//! the built-in tool registry. The remaining session fields are intentionally
//! outside this projection.
//!
//! The protocol crate at this stack tip does not yet expose these TypeScript
//! schemas as Rust values. The small raw-value constructors below therefore
//! let an adapter preserve the exact permission value while making unknown,
//! missing, and malformed sandbox values fail closed.

#![allow(clippy::module_name_repetitions)]

/// Permission modes currently accepted by `ThreadSessionPolicy`.
///
/// The `Unknown` variant keeps a raw value available at the adapter boundary
/// without treating it as a different known permission mode. This preserves
/// the TypeScript policy's direct `permission_mode` passthrough.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum PermissionMode {
    /// Sensitive tool calls never receive an approval prompt.
    Never,
    /// Sensitive tool calls may receive an approval prompt.
    OnRequest,
    /// A permission literal introduced by a newer or malformed input.
    Unknown(String),
}

impl PermissionMode {
    /// Every permission mode currently defined by the protocol, in order.
    pub const ALL: [Self; 2] = [Self::Never, Self::OnRequest];

    /// Classifies an exact raw permission literal without normalization.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "never" => Self::Never,
            "on_request" => Self::OnRequest,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Returns the exact raw permission literal represented by this value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Never => "never",
            Self::OnRequest => "on_request",
            Self::Unknown(raw) => raw,
        }
    }

    /// Converts this session permission to the output approval value.
    #[must_use]
    pub fn approval(self) -> ApprovalMode {
        match self {
            Self::Never => ApprovalMode::Never,
            Self::OnRequest => ApprovalMode::OnRequest,
            Self::Unknown(raw) => ApprovalMode::Unknown(raw),
        }
    }
}

/// Sandbox modes currently accepted by `ThreadSessionPolicy`.
///
/// Unknown values are retained for diagnostics and are never considered
/// mutating. A missing sandbox mode is represented by `None` on
/// [`SessionPolicyView`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SandboxMode {
    /// The session cannot mutate the workspace.
    ReadOnly,
    /// The session may use the workspace-mutating tool capabilities.
    WorkspaceWrite,
    /// A sandbox literal introduced by a newer or malformed input.
    Unknown(String),
}

impl SandboxMode {
    /// Every sandbox mode currently defined by the protocol, in order.
    pub const ALL: [Self; 2] = [Self::ReadOnly, Self::WorkspaceWrite];

    /// Classifies an exact raw sandbox literal without normalization.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "read_only" => Self::ReadOnly,
            "workspace_write" => Self::WorkspaceWrite,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Returns the exact raw sandbox literal represented by this value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::ReadOnly => "read_only",
            Self::WorkspaceWrite => "workspace_write",
            Self::Unknown(raw) => raw,
        }
    }

    /// Returns whether this exact mode authorizes mutating capabilities.
    #[must_use]
    pub fn allows_mutation(&self) -> bool {
        matches!(self, Self::WorkspaceWrite)
    }
}

/// The minimal session-policy projection consumed by tool permission mapping.
///
/// The complete `ThreadSessionPolicy` contains engine, model, catalog,
/// reasoning, service, and feature fields. None of those fields affect this
/// policy. The two options also make a malformed or partially decoded policy
/// representable without inventing a default sandbox scope.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SessionPolicyView {
    /// The raw permission/approval mode, when the policy supplied one.
    pub permission_mode: Option<PermissionMode>,
    /// The raw sandbox scope, when the policy supplied one.
    pub sandbox_mode: Option<SandboxMode>,
}

impl SessionPolicyView {
    /// Builds a view from classified permission and sandbox modes.
    #[must_use]
    pub const fn new(permission_mode: PermissionMode, sandbox_mode: SandboxMode) -> Self {
        Self {
            permission_mode: Some(permission_mode),
            sandbox_mode: Some(sandbox_mode),
        }
    }

    /// Builds a view by classifying the exact raw fields supplied by an
    /// adapter. `None` preserves an omitted field rather than guessing.
    #[must_use]
    pub fn from_raw(permission_mode: Option<&str>, sandbox_mode: Option<&str>) -> Self {
        Self {
            permission_mode: permission_mode.map(PermissionMode::from_raw),
            sandbox_mode: sandbox_mode.map(SandboxMode::from_raw),
        }
    }
}

/// Approval values accepted by `ArtisanToolPermissionPolicy`.
///
/// `Unknown` is retained only so a raw adapter input can be observed without
/// rewriting it. A schema-valid `ThreadSessionPolicy` produces `Never` or
/// `OnRequest`; `Always` remains part of the output protocol vocabulary even
/// though this mapper never selects it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ApprovalMode {
    /// The tool may run without an approval prompt.
    Never,
    /// A sensitive tool may require an approval prompt.
    OnRequest,
    /// The tool always requires an approval prompt.
    Always,
    /// An unrecognized raw permission value passed through exactly.
    Unknown(String),
}

impl ApprovalMode {
    /// Every output approval value defined by the protocol.
    pub const ALL: [Self; 3] = [Self::Never, Self::OnRequest, Self::Always];

    /// Classifies an exact raw approval literal without normalization.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "never" => Self::Never,
            "on_request" => Self::OnRequest,
            "always" => Self::Always,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Returns the exact raw approval literal represented by this value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Never => "never",
            Self::OnRequest => "on_request",
            Self::Always => "always",
            Self::Unknown(raw) => raw,
        }
    }
}

/// The seven tool-permission fields consumed by built-in tool discovery.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtisanToolPermissionPolicy {
    /// Approval behavior for tools that declare a sensitive requirement.
    pub approval: ApprovalMode,
    /// Whether engine-owned actions may be observed.
    pub allow_engine_observation: bool,
    /// Whether Git index writes are allowed.
    pub allow_git_index_write: bool,
    /// Whether preview control is allowed.
    pub allow_preview_control: bool,
    /// Whether process control is allowed.
    pub allow_process_control: bool,
    /// Whether workspace reads are allowed.
    pub allow_workspace_read: bool,
    /// Whether workspace writes are allowed.
    pub allow_workspace_write: bool,
}

/// Maps a session policy projection to built-in tool permissions.
///
/// This mirrors `MakeSessionToolPolicy` exactly:
///
/// - approval is the policy's exact permission mode, or `never` when absent;
/// - engine observation and workspace reads are always allowed;
/// - Git index, preview, process, and workspace writes are allowed only when
///   the sandbox value is exactly `workspace_write`;
/// - absent, read-only, unknown, and malformed sandbox values fail closed.
#[must_use]
pub fn make_session_tool_policy(policy: Option<SessionPolicyView>) -> ArtisanToolPermissionPolicy {
    let (approval, allow_mutation) = match policy {
        Some(policy) => (
            policy
                .permission_mode
                .map_or(ApprovalMode::Never, PermissionMode::approval),
            policy
                .sandbox_mode
                .is_some_and(|sandbox| sandbox.allows_mutation()),
        ),
        None => (ApprovalMode::Never, false),
    };

    ArtisanToolPermissionPolicy {
        approval,
        allow_engine_observation: true,
        allow_git_index_write: allow_mutation,
        allow_preview_control: allow_mutation,
        allow_process_control: allow_mutation,
        allow_workspace_read: true,
        allow_workspace_write: allow_mutation,
    }
}
