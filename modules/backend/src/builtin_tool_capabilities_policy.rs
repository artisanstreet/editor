//! Deterministic capability resolution for the built-in backend tools.
//!
//! The TypeScript implementation asks three workspace-scoped registries for a
//! capability, but the policy itself does not need to know how those registries
//! work.  This module therefore accepts typed lookup outcomes and only models
//! the observable availability decision.  It performs no filesystem, process,
//! registry, or other external I/O.

/// The exact reason used when preview tools have no configured adapter.
pub const PREVIEW_UNAVAILABLE_REASON: &str = "No preview adapter is configured";

/// The exact reason used when the backend has no language-service provider.
pub const LANGUAGE_SERVICE_UNAVAILABLE_REASON: &str = "No backend language service is configured";

/// The exact reason used when a workspace-scoped tool has no workspace.
pub const WORKSPACE_REQUIRED_REASON: &str = "A registered workspace is required";

/// The exact reason used when a workspace registry lookup cannot provide a capability.
pub const WORKSPACE_CAPABILITY_UNAVAILABLE_REASON: &str = "Workspace capability is not registered";

/// The built-in tool identifiers that use the bounded regular-file store registry.
pub const FILE_STORE_TOOL_IDS: &[&str] = &["workspace.file.read", "workspace.file.write"];

/// The built-in tool identifiers that use the workspace filesystem registry.
pub const FILESYSTEM_TOOL_IDS: &[&str] = &["workspace.file.list", "terminal.open"];

/// The built-in tool identifiers that use the workspace Git registry.
pub const GIT_TOOL_IDS: &[&str] = &[
    "git.status.read",
    "git.diff.read",
    "git.index.stage",
    "git.index.unstage",
];

/// Owns an exact built-in tool identifier while allowing arbitrary IDs for
/// deterministic policy tests and future catalog additions.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ToolId(String);

impl ToolId {
    /// Creates a tool identifier without changing its contents.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrows the exact identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the owned identifier text.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl AsRef<str> for ToolId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl From<&str> for ToolId {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for ToolId {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl std::fmt::Display for ToolId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The local Rust counterpart of the protocol's opaque Artisan tool ID.
pub type ArtisanToolId = ToolId;

/// Identifies the workspace registry selected for a tool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryKind {
    FileStore,
    Filesystem,
    Git,
}

/// Describes the only registry facts needed by this policy.
///
/// `Missing` and `Failed` intentionally remain distinct inputs even though
/// the public resolution reason is the same.  This lets callers and tests
/// model both direct absence and an unsuccessful lookup without introducing
/// any registry implementation or side effect here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryOutcome {
    Present,
    Missing,
    Failed,
}

/// Injected outcomes for the three workspace capability registries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistryOutcomes {
    pub file_store: RegistryOutcome,
    pub filesystem: RegistryOutcome,
    pub git: RegistryOutcome,
}

impl RegistryOutcomes {
    /// Creates a complete typed registry snapshot.
    #[must_use]
    pub const fn new(
        file_store: RegistryOutcome,
        filesystem: RegistryOutcome,
        git: RegistryOutcome,
    ) -> Self {
        Self {
            file_store,
            filesystem,
            git,
        }
    }

    /// Creates a snapshot with one outcome for every registry.
    #[must_use]
    pub const fn all(outcome: RegistryOutcome) -> Self {
        Self::new(outcome, outcome, outcome)
    }
}

impl Default for RegistryOutcomes {
    fn default() -> Self {
        Self::all(RegistryOutcome::Missing)
    }
}

/// Compatibility alias emphasizing that the outcomes are lookup results.
pub type RegistryLookup = RegistryOutcome;

/// The availability state in a resolved capability value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityState {
    Available,
    Unavailable,
}

/// The side-effect-free result of resolving one built-in tool capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCapabilityResolution {
    pub state: CapabilityState,
    pub tool_id: ToolId,
    pub unavailable_reason: Option<&'static str>,
}

impl ToolCapabilityResolution {
    fn available(tool_id: ToolId) -> Self {
        Self {
            state: CapabilityState::Available,
            tool_id,
            unavailable_reason: None,
        }
    }

    fn unavailable(tool_id: ToolId, reason: &'static str) -> Self {
        Self {
            state: CapabilityState::Unavailable,
            tool_id,
            unavailable_reason: Some(reason),
        }
    }

    /// Whether the resolved capability is available.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self.state, CapabilityState::Available)
    }

    /// The exact unavailable reason, if the capability is unavailable.
    #[must_use]
    pub const fn unavailable_reason(&self) -> Option<&'static str> {
        self.unavailable_reason
    }
}

/// Alias matching the service's capability-state terminology.
pub type CapabilityResolution = ToolCapabilityResolution;

/// Alias matching the built-in service's public state name.
pub type ArtisanToolCapabilityState = ToolCapabilityResolution;

/// Stateless resolver for built-in workspace capability availability.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BuiltInToolCapabilityPolicy {
    registries: RegistryOutcomes,
}

impl BuiltInToolCapabilityPolicy {
    /// Creates a resolver from injected registry lookup outcomes.
    #[must_use]
    pub const fn new(registries: RegistryOutcomes) -> Self {
        Self { registries }
    }

    /// Returns the injected outcomes retained by this policy.
    #[must_use]
    pub const fn registries(self) -> RegistryOutcomes {
        self.registries
    }

    /// Resolves one owned tool ID using the TypeScript policy's exact order.
    ///
    /// The `workspace_id` option is presence-only.  In particular, `Some("")`
    /// is present just as any defined JavaScript string is present; the ID is
    /// not inspected, normalized, or copied into the result.
    #[must_use]
    pub fn resolve(self, tool_id: ToolId, workspace_id: Option<&str>) -> ToolCapabilityResolution {
        resolve_with_registries(tool_id, workspace_id, self.registries)
    }

    /// Resolves one tool using a method name parallel to the source registry's `Get`.
    #[must_use]
    pub fn get(self, tool_id: ToolId, workspace_id: Option<&str>) -> ToolCapabilityResolution {
        self.resolve(tool_id, workspace_id)
    }
}

/// Alias using the conventional Rust spelling without the `In` infix.
pub type BuiltinToolCapabilityPolicy = BuiltInToolCapabilityPolicy;

/// Selects the first matching workspace registry for a tool ID.
///
/// The sets are disjoint today, but the ordered match deliberately mirrors
/// the source's file-store, filesystem, then Git selection precedence.
#[must_use]
pub fn selected_registry(tool_id: &ToolId) -> Option<RegistryKind> {
    match tool_id.as_str() {
        "workspace.file.read" | "workspace.file.write" => Some(RegistryKind::FileStore),
        "workspace.file.list" | "terminal.open" => Some(RegistryKind::Filesystem),
        "git.status.read" | "git.diff.read" | "git.index.stage" | "git.index.unstage" => {
            Some(RegistryKind::Git)
        }
        _ => None,
    }
}

/// Alias emphasizing that this is the registry required by a tool.
#[must_use]
pub fn required_registry(tool_id: &ToolId) -> Option<RegistryKind> {
    selected_registry(tool_id)
}

/// Resolves one tool ID using explicitly supplied registry outcomes.
#[must_use]
pub fn resolve_with_registries(
    tool_id: ToolId,
    workspace_id: Option<&str>,
    registries: RegistryOutcomes,
) -> ToolCapabilityResolution {
    if tool_id.as_str().starts_with("preview.") {
        return ToolCapabilityResolution::unavailable(tool_id, PREVIEW_UNAVAILABLE_REASON);
    }

    if tool_id.as_str() == "workspace.language.status" {
        return ToolCapabilityResolution::unavailable(tool_id, LANGUAGE_SERVICE_UNAVAILABLE_REASON);
    }

    let Some(registry) = selected_registry(&tool_id) else {
        return ToolCapabilityResolution::available(tool_id);
    };

    if workspace_id.is_none() {
        return ToolCapabilityResolution::unavailable(tool_id, WORKSPACE_REQUIRED_REASON);
    }

    let outcome = match registry {
        RegistryKind::FileStore => registries.file_store,
        RegistryKind::Filesystem => registries.filesystem,
        RegistryKind::Git => registries.git,
    };

    match outcome {
        RegistryOutcome::Present => ToolCapabilityResolution::available(tool_id),
        RegistryOutcome::Missing | RegistryOutcome::Failed => {
            ToolCapabilityResolution::unavailable(tool_id, WORKSPACE_CAPABILITY_UNAVAILABLE_REASON)
        }
    }
}

/// Convenience entry point for callers that do not need a policy value.
#[must_use]
pub fn resolve(
    tool_id: ToolId,
    workspace_id: Option<&str>,
    registries: RegistryOutcomes,
) -> ToolCapabilityResolution {
    resolve_with_registries(tool_id, workspace_id, registries)
}
