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
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when `value` is zero or exceeds
    /// [`ENGINE_RUNTIME_MAX_MILLIS`].
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
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when `value` is zero or exceeds
    /// [`ENGINE_RUNTIME_MAX_BODY_BYTES`].
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
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when `value` is zero or exceeds
    /// [`ENGINE_RUNTIME_MAX_OBSERVATIONS`].
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
///
/// Every user-facing engine is representable in configuration. Only
/// [`EngineId::OpenCode2`] has an execution runtime in this packet; the other
/// variants persist, validate, and cross the wire, but dispatch rejects them
/// instead of running them as another engine. ACP is a transport shared by
/// several engines, never a seventh user-facing engine.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EngineId {
    /// The first supported native engine implementation.
    OpenCode2,
    /// Codex CLI (`codex app-server` over stdio JSON-RPC).
    Codex,
    /// Claude Code CLI (`claude --output-format stream-json` over stdio).
    Claude,
    /// Grok Build through the shared ACP stdio transport.
    Grok,
    /// Cursor through the shared ACP stdio transport.
    Cursor,
    /// Hermes private-service gateway over WebSocket JSON-RPC.
    Hermes,
}

impl EngineId {
    /// Every stable engine identity.
    pub const ALL: [Self; 6] = [
        Self::OpenCode2,
        Self::Codex,
        Self::Claude,
        Self::Grok,
        Self::Cursor,
        Self::Hermes,
    ];

    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenCode2 => "opencode2",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Grok => "grok",
            Self::Cursor => "cursor",
            Self::Hermes => "hermes",
        }
    }

    /// Parses a stable storage or wire spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown spelling. Unknown engines never default to another
    /// engine.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "opencode2" => Ok(Self::OpenCode2),
            "codex" => Ok(Self::Codex),
            "claude" => Ok(Self::Claude),
            "grok" => Ok(Self::Grok),
            "cursor" => Ok(Self::Cursor),
            "hermes" => Ok(Self::Hermes),
            _ => Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported,
            )),
        }
    }

    /// Whether Forge can execute runs for this engine yet.
    ///
    /// Only `OpenCode2` has a runtime in this packet. Every other engine is
    /// representable but explicitly unrunnable until its runtime packet lands;
    /// dispatch must reject it rather than run it as `OpenCode2`.
    #[must_use]
    pub const fn has_execution_runtime(self) -> bool {
        matches!(self, Self::OpenCode2)
    }
}

/// Engine-specific selection values.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EngineSelection {
    /// `OpenCode` 2 selection.
    OpenCode2(OpenCode2Selection),
    /// Codex CLI selection.
    Codex(CodexSelection),
    /// Claude Code CLI selection.
    Claude(ClaudeSelection),
    /// Grok Build selection (shared ACP transport).
    Grok(GrokSelection),
    /// Cursor selection (shared ACP transport).
    Cursor(CursorSelection),
    /// Hermes gateway selection.
    Hermes(HermesSelection),
}

impl EngineSelection {
    /// Returns the selected engine implementation.
    #[must_use]
    pub const fn engine_id(&self) -> EngineId {
        match self {
            Self::OpenCode2(_) => EngineId::OpenCode2,
            Self::Codex(_) => EngineId::Codex,
            Self::Claude(_) => EngineId::Claude,
            Self::Grok(_) => EngineId::Grok,
            Self::Cursor(_) => EngineId::Cursor,
            Self::Hermes(_) => EngineId::Hermes,
        }
    }

    /// Returns the managed profile identity shared by every engine selection.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        match self {
            Self::OpenCode2(selection) => selection.profile_id(),
            Self::Codex(selection) => selection.profile_id(),
            Self::Claude(selection) => selection.profile_id(),
            Self::Grok(selection) => selection.profile_id(),
            Self::Cursor(selection) => selection.profile_id(),
            Self::Hermes(selection) => selection.profile_id(),
        }
    }

    /// Returns the canonical permission policy when the engine uses one.
    ///
    /// Hermes owns authorization through its installed profile and carries
    /// its own [`HermesPermissionMode`] instead, so this is `None` for
    /// Hermes and `Some` for every other engine.
    #[must_use]
    pub const fn permission(&self) -> Option<&EnginePermissionPolicy> {
        match self {
            Self::OpenCode2(selection) => Some(selection.permission()),
            Self::Codex(selection) => Some(selection.permission()),
            Self::Claude(selection) => Some(selection.permission()),
            Self::Grok(selection) => Some(selection.permission()),
            Self::Cursor(selection) => Some(selection.permission()),
            Self::Hermes(_) => None,
        }
    }

    /// Returns the CLI-facing model identity when the selection names one.
    #[must_use]
    pub const fn model_id(&self) -> Option<&EngineModelId> {
        match self {
            Self::OpenCode2(selection) => Some(selection.model_id()),
            Self::Codex(selection) => selection.model_id(),
            Self::Claude(selection) => selection.model_id(),
            Self::Grok(selection) => selection.model_id(),
            Self::Cursor(selection) => selection.model_id(),
            Self::Hermes(selection) => Some(selection.model_id()),
        }
    }

    /// Whether Forge can execute this selection yet (`OpenCode` 2 only in
    /// this packet). Anything else must be rejected, never coerced.
    #[must_use]
    pub const fn is_execution_supported(&self) -> bool {
        matches!(self, Self::OpenCode2(_))
    }

    /// Borrows the `OpenCode` 2 selection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// when the selection names another engine. Every production caller
    /// handles this; newly representable engines never coerce to `OpenCode`
    /// 2 and never panic here.
    pub const fn as_opencode2(&self) -> Result<&OpenCode2Selection, EngineConfigError> {
        match self {
            Self::OpenCode2(selection) => Ok(selection),
            Self::Codex(_)
            | Self::Claude(_)
            | Self::Grok(_)
            | Self::Cursor(_)
            | Self::Hermes(_) => Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported,
            )),
        }
    }

    /// Borrows the Codex selection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// when the selection names another engine.
    pub const fn as_codex(&self) -> Result<&CodexSelection, EngineConfigError> {
        match self {
            Self::Codex(selection) => Ok(selection),
            Self::OpenCode2(_)
            | Self::Claude(_)
            | Self::Grok(_)
            | Self::Cursor(_)
            | Self::Hermes(_) => Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported,
            )),
        }
    }

    /// Borrows the Claude selection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// when the selection names another engine.
    pub const fn as_claude(&self) -> Result<&ClaudeSelection, EngineConfigError> {
        match self {
            Self::Claude(selection) => Ok(selection),
            Self::OpenCode2(_)
            | Self::Codex(_)
            | Self::Grok(_)
            | Self::Cursor(_)
            | Self::Hermes(_) => Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported,
            )),
        }
    }

    /// Borrows the Grok selection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// when the selection names another engine.
    pub const fn as_grok(&self) -> Result<&GrokSelection, EngineConfigError> {
        match self {
            Self::Grok(selection) => Ok(selection),
            Self::OpenCode2(_)
            | Self::Codex(_)
            | Self::Claude(_)
            | Self::Cursor(_)
            | Self::Hermes(_) => Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported,
            )),
        }
    }

    /// Borrows the Cursor selection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// when the selection names another engine.
    pub const fn as_cursor(&self) -> Result<&CursorSelection, EngineConfigError> {
        match self {
            Self::Cursor(selection) => Ok(selection),
            Self::OpenCode2(_)
            | Self::Codex(_)
            | Self::Claude(_)
            | Self::Grok(_)
            | Self::Hermes(_) => Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported,
            )),
        }
    }

    /// Borrows the Hermes selection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// when the selection names another engine.
    pub const fn as_hermes(&self) -> Result<&HermesSelection, EngineConfigError> {
        match self {
            Self::Hermes(selection) => Ok(selection),
            Self::OpenCode2(_)
            | Self::Codex(_)
            | Self::Claude(_)
            | Self::Grok(_)
            | Self::Cursor(_) => Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Complete selection for the `OpenCode` 2 engine.
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

/// Maximum UTF-8 byte length of one open-ended reasoning-effort label.
///
/// The Grok, Cursor, and Hermes adapters accept provider-defined effort
/// labels as non-empty strings. Sixty-four bytes leaves generous room for
/// compound labels such as `xhigh-fast` while keeping the durable encoding
/// bounded.
const REASONING_EFFORT_MAX_BYTES: usize = 64;

/// Maximum model context window override accepted in tokens.
///
/// The TypeScript Codex adapter requires only a positive safe integer string
/// for `provider_options.codex.model_context_window`. The native bound keeps
/// that positivity rule and adds a finite ceiling far above any realistic
/// window so a corrupt value cannot inflate durable bounds.
const CODEX_MODEL_CONTEXT_WINDOW_MAX_TOKENS: u64 = 100_000_000;

/// Reasoning-effort choice accepted by the Codex adapter
/// (`provider_options.codex.reasoning_effort`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CodexReasoningEffort {
    /// Low reasoning effort.
    Low,
    /// Medium reasoning effort.
    Medium,
    /// High reasoning effort.
    High,
    /// Extra-high reasoning effort.
    XHigh,
    /// Maximum reasoning effort.
    Max,
    /// Ultra reasoning effort.
    Ultra,
}

impl CodexReasoningEffort {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
            Self::Ultra => "ultra",
        }
    }

    /// Parses the adapter spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown label.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            "ultra" => Ok(Self::Ultra),
            _ => Err(EngineConfigError::new(
                "reasoning_effort",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Service-tier choice accepted by the Codex adapter
/// (`provider_options.codex.service_tier`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CodexServiceTier {
    /// Standard service tier.
    Standard,
    /// Fast service tier.
    Fast,
}

impl CodexServiceTier {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Fast => "fast",
        }
    }

    /// Parses the adapter spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown label.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "standard" => Ok(Self::Standard),
            "fast" => Ok(Self::Fast),
            _ => Err(EngineConfigError::new(
                "service_tier",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Positive model-context-window override in tokens
/// (`provider_options.codex.model_context_window`).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CodexModelContextWindow(u64);

impl CodexModelContextWindow {
    /// Creates a window override in the inclusive
    /// `1..=CODEX_MODEL_CONTEXT_WINDOW_MAX_TOKENS` range.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when `value` is zero or exceeds the
    /// finite ceiling.
    pub const fn new(value: u64) -> Result<Self, EngineConfigError> {
        if value == 0 || value > CODEX_MODEL_CONTEXT_WINDOW_MAX_TOKENS {
            Err(EngineConfigError::new(
                "model_context_window",
                EngineConfigReason::OutOfRange,
            ))
        } else {
            Ok(Self(value))
        }
    }

    /// Returns the window override in tokens.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Complete selection for the Codex engine.
///
/// The model stays optional because the adapter omits `--model` when no
/// model is selected. The permission rules mirror
/// `codex/internal/permissions.ts`: `always` approval is rejected, network
/// access requires write access, and host scope requires network access.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CodexSelection {
    profile_id: EngineProfileId,
    model_id: Option<EngineModelId>,
    permission: EnginePermissionPolicy,
    reasoning_effort: Option<CodexReasoningEffort>,
    service_tier: Option<CodexServiceTier>,
    model_context_window: Option<CodexModelContextWindow>,
}

impl CodexSelection {
    /// Constructs a complete Codex selection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Inconsistent`]
    /// when the permission policy violates a Codex adapter relationship
    /// (`always` approval, network access without write access, or host
    /// scope without network access).
    pub fn new(
        profile_id: EngineProfileId,
        model_id: Option<EngineModelId>,
        permission: EnginePermissionPolicy,
        reasoning_effort: Option<CodexReasoningEffort>,
        service_tier: Option<CodexServiceTier>,
        model_context_window: Option<CodexModelContextWindow>,
    ) -> Result<Self, EngineConfigError> {
        if permission.approval() == ApprovalMode::Always {
            return Err(EngineConfigError::new(
                "approval",
                EngineConfigReason::Inconsistent,
            ));
        }
        let write_access = permission.filesystem() != FilesystemAccess::None;
        if permission.network() == NetworkAccess::Enabled && !write_access {
            return Err(EngineConfigError::new(
                "network",
                EngineConfigReason::Inconsistent,
            ));
        }
        if permission.filesystem() == FilesystemAccess::Host
            && permission.network() != NetworkAccess::Enabled
        {
            return Err(EngineConfigError::new(
                "network",
                EngineConfigReason::Inconsistent,
            ));
        }
        Ok(Self {
            profile_id,
            model_id,
            permission,
            reasoning_effort,
            service_tier,
            model_context_window,
        })
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the optional model identity.
    #[must_use]
    pub const fn model_id(&self) -> Option<&EngineModelId> {
        self.model_id.as_ref()
    }

    /// Returns the permission policy.
    #[must_use]
    pub const fn permission(&self) -> &EnginePermissionPolicy {
        &self.permission
    }

    /// Returns the optional reasoning-effort choice.
    #[must_use]
    pub const fn reasoning_effort(&self) -> Option<CodexReasoningEffort> {
        self.reasoning_effort
    }

    /// Returns the optional service-tier choice.
    #[must_use]
    pub const fn service_tier(&self) -> Option<CodexServiceTier> {
        self.service_tier
    }

    /// Returns the optional model-context-window override.
    #[must_use]
    pub const fn model_context_window(&self) -> Option<CodexModelContextWindow> {
        self.model_context_window
    }
}

/// Effort choice accepted by the Claude adapter
/// (`provider_options.claude.effort`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClaudeEffort {
    /// Low effort.
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
    /// Extra-high effort.
    XHigh,
    /// Maximum effort.
    Max,
}

impl ClaudeEffort {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// Parses the adapter spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown label.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::XHigh),
            "max" => Ok(Self::Max),
            _ => Err(EngineConfigError::new(
                "effort",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Native permission mode accepted by the Claude adapter
/// (`provider_options.claude.permission_mode`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ClaudePermissionMode {
    /// Plan mode.
    Plan,
    /// Default mode.
    Default,
    /// Accept-edits mode.
    AcceptEdits,
    /// Automatic mode.
    Auto,
    /// Bypass-permissions mode.
    BypassPermissions,
    /// Manual mode.
    Manual,
    /// Do-not-ask mode.
    DontAsk,
}

impl ClaudePermissionMode {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Auto => "auto",
            Self::BypassPermissions => "bypassPermissions",
            Self::Manual => "manual",
            Self::DontAsk => "dontAsk",
        }
    }

    /// Parses the adapter spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown label.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "plan" => Ok(Self::Plan),
            "default" => Ok(Self::Default),
            "acceptEdits" => Ok(Self::AcceptEdits),
            "auto" => Ok(Self::Auto),
            "bypassPermissions" => Ok(Self::BypassPermissions),
            "manual" => Ok(Self::Manual),
            "dontAsk" => Ok(Self::DontAsk),
            _ => Err(EngineConfigError::new(
                "permission_mode",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Complete selection for the Claude engine.
///
/// The model stays optional because the adapter passes `--model` only when a
/// model is selected. The permission rules mirror `claude/cli-engine.ts`:
/// the CLI always needs write and network access. The append-system-prompt
/// file option carries a host path, so it stays a launch-time concern and is
/// deliberately not part of the durable selection.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ClaudeSelection {
    profile_id: EngineProfileId,
    model_id: Option<EngineModelId>,
    permission: EnginePermissionPolicy,
    effort: Option<ClaudeEffort>,
    permission_mode: Option<ClaudePermissionMode>,
    disable_tools: bool,
    safe_mode: bool,
}

impl ClaudeSelection {
    /// Constructs a complete Claude selection.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Inconsistent`]
    /// when the permission policy denies write or network access, which the
    /// Claude adapter does not support.
    pub fn new(
        profile_id: EngineProfileId,
        model_id: Option<EngineModelId>,
        permission: EnginePermissionPolicy,
        effort: Option<ClaudeEffort>,
        permission_mode: Option<ClaudePermissionMode>,
        disable_tools: bool,
        safe_mode: bool,
    ) -> Result<Self, EngineConfigError> {
        if permission.filesystem() == FilesystemAccess::None {
            return Err(EngineConfigError::new(
                "filesystem",
                EngineConfigReason::Inconsistent,
            ));
        }
        if permission.network() != NetworkAccess::Enabled {
            return Err(EngineConfigError::new(
                "network",
                EngineConfigReason::Inconsistent,
            ));
        }
        Ok(Self {
            profile_id,
            model_id,
            permission,
            effort,
            permission_mode,
            disable_tools,
            safe_mode,
        })
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the optional model identity.
    #[must_use]
    pub const fn model_id(&self) -> Option<&EngineModelId> {
        self.model_id.as_ref()
    }

    /// Returns the permission policy.
    #[must_use]
    pub const fn permission(&self) -> &EnginePermissionPolicy {
        &self.permission
    }

    /// Returns the optional effort choice.
    #[must_use]
    pub const fn effort(&self) -> Option<ClaudeEffort> {
        self.effort
    }

    /// Returns the optional native permission mode.
    #[must_use]
    pub const fn permission_mode(&self) -> Option<ClaudePermissionMode> {
        self.permission_mode
    }

    /// Returns whether tools are disabled (`claude.disable_tools`).
    #[must_use]
    pub const fn disable_tools(&self) -> bool {
        self.disable_tools
    }

    /// Returns whether safe mode is enabled (`claude.safe_mode`).
    #[must_use]
    pub const fn safe_mode(&self) -> bool {
        self.safe_mode
    }
}

/// Permission mode accepted by the Grok adapter
/// (`provider_options.grok.permission_mode`). A read-only canonical policy
/// additionally maps to the CLI plan mode, mirroring `grok/engine.ts`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GrokPermissionMode {
    /// Automatic permission mode.
    Auto,
    /// Always-approve permission mode.
    AlwaysApprove,
}

impl GrokPermissionMode {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AlwaysApprove => "always-approve",
        }
    }

    /// Parses the adapter spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown label.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "auto" => Ok(Self::Auto),
            "always-approve" => Ok(Self::AlwaysApprove),
            _ => Err(EngineConfigError::new(
                "permission_mode",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Provider-defined reasoning-effort label for the Grok adapter
/// (`provider_options.grok.reasoning_effort`, passed to `--reasoning-effort`).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GrokReasoningEffort(String);

impl GrokReasoningEffort {
    /// Parses a non-empty, bounded effort label.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when the label is empty, exceeds
    /// [`REASONING_EFFORT_MAX_BYTES`] bytes, or contains whitespace or
    /// control characters.
    pub fn parse(value: impl Into<String>) -> Result<Self, EngineConfigError> {
        let value = value.into();
        if value.is_empty() || value.len() > REASONING_EFFORT_MAX_BYTES {
            return Err(EngineConfigError::new(
                "reasoning_effort",
                EngineConfigReason::OutOfRange,
            ));
        }
        if value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(EngineConfigError::new(
                "reasoning_effort",
                EngineConfigReason::InvalidIdentifier,
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated effort label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Complete selection for the Grok engine.
///
/// The model stays optional because the adapter omits `--model` when no
/// model is selected. The adapter imposes no construction-time permission
/// rejections beyond the typed options: a read-only policy maps to plan
/// mode at argument-building time.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GrokSelection {
    profile_id: EngineProfileId,
    model_id: Option<EngineModelId>,
    permission: EnginePermissionPolicy,
    reasoning_effort: Option<GrokReasoningEffort>,
    permission_mode: Option<GrokPermissionMode>,
}

impl GrokSelection {
    /// Constructs a complete Grok selection.
    #[must_use]
    pub fn new(
        profile_id: EngineProfileId,
        model_id: Option<EngineModelId>,
        permission: EnginePermissionPolicy,
        reasoning_effort: Option<GrokReasoningEffort>,
        permission_mode: Option<GrokPermissionMode>,
    ) -> Self {
        Self {
            profile_id,
            model_id,
            permission,
            reasoning_effort,
            permission_mode,
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the optional model identity.
    #[must_use]
    pub const fn model_id(&self) -> Option<&EngineModelId> {
        self.model_id.as_ref()
    }

    /// Returns the permission policy.
    #[must_use]
    pub const fn permission(&self) -> &EnginePermissionPolicy {
        &self.permission
    }

    /// Returns the optional reasoning-effort label.
    #[must_use]
    pub const fn reasoning_effort(&self) -> Option<&GrokReasoningEffort> {
        self.reasoning_effort.as_ref()
    }

    /// Returns the optional permission mode.
    #[must_use]
    pub const fn permission_mode(&self) -> Option<GrokPermissionMode> {
        self.permission_mode
    }
}

/// Delivery-speed choice accepted by the Cursor adapter
/// (`provider_options.cursor.speed`, appended as `-fast`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CursorSpeed {
    /// Fast delivery mode.
    Fast,
}

impl CursorSpeed {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
        }
    }

    /// Parses the adapter spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown label.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "fast" => Ok(Self::Fast),
            _ => Err(EngineConfigError::new(
                "speed",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Permission mode accepted by the Cursor adapter
/// (`provider_options.cursor.permission_mode`). A read-only canonical policy
/// additionally maps to `--mode ask`, mirroring `cursor/engine.ts`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CursorPermissionMode {
    /// Force mode.
    Force,
}

impl CursorPermissionMode {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Force => "force",
        }
    }

    /// Parses the adapter spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown label.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "force" => Ok(Self::Force),
            _ => Err(EngineConfigError::new(
                "permission_mode",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Provider-defined reasoning-effort label for the Cursor adapter
/// (`provider_options.cursor.reasoning_effort`, resolved into the exact
/// model id by `ResolveCursorModel` at argument-building time).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CursorReasoningEffort(String);

impl CursorReasoningEffort {
    /// Parses a non-empty, bounded effort label.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when the label is empty, exceeds
    /// [`REASONING_EFFORT_MAX_BYTES`] bytes, or contains whitespace or
    /// control characters.
    pub fn parse(value: impl Into<String>) -> Result<Self, EngineConfigError> {
        let value = value.into();
        if value.is_empty() || value.len() > REASONING_EFFORT_MAX_BYTES {
            return Err(EngineConfigError::new(
                "reasoning_effort",
                EngineConfigReason::OutOfRange,
            ));
        }
        if value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(EngineConfigError::new(
                "reasoning_effort",
                EngineConfigReason::InvalidIdentifier,
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated effort label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Complete selection for the Cursor engine.
///
/// The model stays optional because the adapter omits `--model` when no
/// model is selected. Effort/speed suffix resolution (`ResolveCursorModel`)
/// is argument-building logic and stays in the runtime packet; the durable
/// selection keeps the raw choices.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CursorSelection {
    profile_id: EngineProfileId,
    model_id: Option<EngineModelId>,
    permission: EnginePermissionPolicy,
    reasoning_effort: Option<CursorReasoningEffort>,
    speed: Option<CursorSpeed>,
    permission_mode: Option<CursorPermissionMode>,
}

impl CursorSelection {
    /// Constructs a complete Cursor selection.
    #[must_use]
    pub fn new(
        profile_id: EngineProfileId,
        model_id: Option<EngineModelId>,
        permission: EnginePermissionPolicy,
        reasoning_effort: Option<CursorReasoningEffort>,
        speed: Option<CursorSpeed>,
        permission_mode: Option<CursorPermissionMode>,
    ) -> Self {
        Self {
            profile_id,
            model_id,
            permission,
            reasoning_effort,
            speed,
            permission_mode,
        }
    }

    /// Returns the managed profile identity.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the optional model identity.
    #[must_use]
    pub const fn model_id(&self) -> Option<&EngineModelId> {
        self.model_id.as_ref()
    }

    /// Returns the permission policy.
    #[must_use]
    pub const fn permission(&self) -> &EnginePermissionPolicy {
        &self.permission
    }

    /// Returns the optional reasoning-effort label.
    #[must_use]
    pub const fn reasoning_effort(&self) -> Option<&CursorReasoningEffort> {
        self.reasoning_effort.as_ref()
    }

    /// Returns the optional delivery-speed choice.
    #[must_use]
    pub const fn speed(&self) -> Option<CursorSpeed> {
        self.speed
    }

    /// Returns the optional permission mode.
    #[must_use]
    pub const fn permission_mode(&self) -> Option<CursorPermissionMode> {
        self.permission_mode
    }
}

/// Permission mode accepted by the Hermes adapter
/// (`provider_options.hermes.permission_mode`, required).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HermesPermissionMode {
    /// Installed-profile permission mode.
    Profile,
    /// Yolo permission mode.
    Yolo,
}

impl HermesPermissionMode {
    /// Returns the stable storage and wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Profile => "profile",
            Self::Yolo => "yolo",
        }
    }

    /// Parses the adapter spelling.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] with [`EngineConfigReason::Unsupported`]
    /// for any unknown label.
    pub fn parse(value: &str) -> Result<Self, EngineConfigError> {
        match value {
            "profile" => Ok(Self::Profile),
            "yolo" => Ok(Self::Yolo),
            _ => Err(EngineConfigError::new(
                "permission_mode",
                EngineConfigReason::Unsupported,
            )),
        }
    }
}

/// Provider-defined reasoning-effort label for the Hermes adapter
/// (`provider_options.hermes.reasoning_effort`, defaulting to `medium` at
/// argument-building time).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HermesReasoningEffort(String);

impl HermesReasoningEffort {
    /// Parses a non-empty, bounded effort label.
    ///
    /// # Errors
    ///
    /// Returns [`EngineConfigError`] when the label is empty, exceeds
    /// [`REASONING_EFFORT_MAX_BYTES`] bytes, or contains whitespace or
    /// control characters.
    pub fn parse(value: impl Into<String>) -> Result<Self, EngineConfigError> {
        let value = value.into();
        if value.is_empty() || value.len() > REASONING_EFFORT_MAX_BYTES {
            return Err(EngineConfigError::new(
                "reasoning_effort",
                EngineConfigReason::OutOfRange,
            ));
        }
        if value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(EngineConfigError::new(
                "reasoning_effort",
                EngineConfigReason::InvalidIdentifier,
            ));
        }
        Ok(Self(value))
    }

    /// Returns the validated effort label.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Complete selection for the Hermes engine.
///
/// Hermes authorization is owned by the installed Hermes profile, so the
/// selection carries the adapter's own [`HermesPermissionMode`] instead of
/// the canonical [`EnginePermissionPolicy`]. Model and route are required
/// because the adapter validates the model selection against the live
/// catalog before opening a session.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HermesSelection {
    profile_id: EngineProfileId,
    model_id: EngineModelId,
    route_id: EngineRouteId,
    permission_mode: HermesPermissionMode,
    reasoning_effort: Option<HermesReasoningEffort>,
    fast: bool,
}

impl HermesSelection {
    /// Constructs a complete Hermes selection.
    #[must_use]
    pub fn new(
        profile_id: EngineProfileId,
        model_id: EngineModelId,
        route_id: EngineRouteId,
        permission_mode: HermesPermissionMode,
        reasoning_effort: Option<HermesReasoningEffort>,
        fast: bool,
    ) -> Self {
        Self {
            profile_id,
            model_id,
            route_id,
            permission_mode,
            reasoning_effort,
            fast,
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

    /// Returns the Hermes permission mode.
    #[must_use]
    pub const fn permission_mode(&self) -> HermesPermissionMode {
        self.permission_mode
    }

    /// Returns the optional reasoning-effort label.
    #[must_use]
    pub const fn reasoning_effort(&self) -> Option<&HermesReasoningEffort> {
        self.reasoning_effort.as_ref()
    }

    /// Returns whether fast delivery is requested (`hermes.fast`).
    #[must_use]
    pub const fn fast(&self) -> bool {
        self.fast
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
            EngineId::Codex
            | EngineId::Claude
            | EngineId::Grok
            | EngineId::Cursor
            | EngineId::Hermes => 2,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn runtime(attempt: u64) -> Result<EngineRuntimeControls, EngineConfigError> {
        let one = FiniteMillis::new(1)?;
        EngineRuntimeControls::new(EngineRuntimeControlsInput {
            attempt_budget: FiniteMillis::new(attempt)?,
            readiness_budget: one,
            health_budget: one,
            prompt_budget: one,
            stream_budget: one,
            close_budget: one,
            max_json_body_bytes: ByteLimit::new(1)?,
            max_sse_line_bytes: ByteLimit::new(1)?,
            max_sse_event_bytes: ByteLimit::new(1)?,
            max_readiness_line_bytes: ByteLimit::new(1)?,
            max_header_count: CountLimit::new(1)?,
            max_http_buffer_bytes: ByteLimit::new(1)?,
            max_stderr_bytes: ByteLimit::new(1)?,
            observation_capacity: CountLimit::new(1)?,
        })
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
        assert!(ByteLimit::new(24 * 1024 * 1024 + 1).is_err());
        assert!(CountLimit::new(0).is_err());
        assert!(CountLimit::new(4_097).is_err());
        assert!(EngineConfigRevision::new(0).is_err());
        assert!(EngineConfigRevision::new(i64::MAX as u64 + 1).is_err());
        assert!(EngineConfigRevision::new(1).is_ok());
    }

    #[test]
    fn revision_sqlite_conversion_preserves_boundary_and_checked_overflow() {
        let maximum =
            EngineConfigRevision::new(i64::MAX as u64).expect("SQLite maximum is a valid revision");
        assert_eq!(maximum.as_i64(), i64::MAX);
        assert!(maximum.checked_next().is_err());
    }

    #[test]
    fn phase_and_containing_buffer_relationships_are_checked() {
        let one = FiniteMillis::new(1).expect("one millisecond is valid");
        let bytes = |value| ByteLimit::new(value).expect("byte limit is valid");
        let count = |value| CountLimit::new(value).expect("count limit is valid");
        assert!(
            EngineRuntimeControls::new(EngineRuntimeControlsInput {
                attempt_budget: FiniteMillis::new(4).expect("attempt budget is valid"),
                readiness_budget: one,
                health_budget: one,
                prompt_budget: one,
                stream_budget: one,
                close_budget: one,
                max_json_body_bytes: bytes(1),
                max_sse_line_bytes: bytes(2),
                max_sse_event_bytes: bytes(1),
                max_readiness_line_bytes: bytes(1),
                max_header_count: count(1),
                max_http_buffer_bytes: bytes(1),
                max_stderr_bytes: bytes(1),
                observation_capacity: count(1),
            })
            .is_err()
        );
        assert!(
            EngineRuntimeControls::new(EngineRuntimeControlsInput {
                attempt_budget: FiniteMillis::new(5).expect("attempt budget is valid"),
                readiness_budget: one,
                health_budget: one,
                prompt_budget: one,
                stream_budget: one,
                close_budget: one,
                max_json_body_bytes: bytes(1),
                max_sse_line_bytes: bytes(2),
                max_sse_event_bytes: bytes(1),
                max_readiness_line_bytes: bytes(1),
                max_header_count: count(1),
                max_http_buffer_bytes: bytes(1),
                max_stderr_bytes: bytes(1),
                observation_capacity: count(1),
            })
            .is_err()
        );
    }

    #[test]
    fn missing_variant_is_explicit_and_default_is_an_ordinary_profile_id() {
        let config = complete_config("default");
        let selection = config
            .selection()
            .as_opencode2()
            .expect("fixture is an OpenCode2 selection");
        assert_eq!(selection.profile_id().as_str(), "default");
        assert!(selection.variant_id().is_none());
    }

    fn test_permission() -> EnginePermissionPolicy {
        EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::Never,
            FilesystemAccess::None,
            NetworkAccess::Disabled,
            WebSearchAccess::Disabled,
        )
    }

    fn writable_permission() -> EnginePermissionPolicy {
        EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::OnRequest,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
            WebSearchAccess::Disabled,
        )
    }

    fn test_profile() -> EngineProfileId {
        EngineProfileId::parse("profile-test").expect("profile id is valid")
    }

    fn test_model() -> EngineModelId {
        EngineModelId::parse("model-test").expect("model id is valid")
    }

    #[test]
    fn engine_id_spellings_parse_and_reject_unknown() {
        for engine in EngineId::ALL {
            assert_eq!(EngineId::parse(engine.as_str()), Ok(engine));
        }
        assert_eq!(
            EngineId::parse("acp"),
            Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported
            ))
        );
        assert_eq!(
            EngineId::parse("opencode"),
            Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported
            ))
        );
        assert_eq!(
            EngineId::parse(""),
            Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported
            ))
        );
    }

    #[test]
    fn only_opencode2_has_an_execution_runtime() {
        for engine in EngineId::ALL {
            assert_eq!(
                engine.has_execution_runtime(),
                engine == EngineId::OpenCode2,
                "unexpected runtime flag for {}",
                engine.as_str()
            );
        }
    }

    #[test]
    fn opencode2_accessor_rejects_other_engines_without_coercion() {
        let codex = EngineSelection::Codex(
            CodexSelection::new(
                test_profile(),
                Some(test_model()),
                test_permission(),
                None,
                None,
                None,
            )
            .expect("codex selection is valid"),
        );
        assert_eq!(codex.engine_id(), EngineId::Codex);
        assert_eq!(
            codex.as_opencode2(),
            Err(EngineConfigError::new(
                "engine",
                EngineConfigReason::Unsupported
            ))
        );
        assert!(!codex.is_execution_supported());
        assert_eq!(codex.profile_id().as_str(), "profile-test");
        assert_eq!(
            codex.model_id().map(EngineModelId::as_str),
            Some("model-test")
        );
        assert!(codex.as_codex().is_ok());
        assert!(codex.as_claude().is_err());
    }

    #[test]
    fn hermes_selection_has_no_canonical_permission_and_requires_model_route() {
        let hermes = EngineSelection::Hermes(HermesSelection::new(
            test_profile(),
            test_model(),
            EngineRouteId::parse("route-test").expect("route id is valid"),
            HermesPermissionMode::Profile,
            None,
            false,
        ));
        assert_eq!(hermes.engine_id(), EngineId::Hermes);
        assert!(hermes.permission().is_none());
        assert!(hermes.as_opencode2().is_err());
        assert!(hermes.as_hermes().is_ok());
        assert_eq!(
            HermesPermissionMode::parse("yolo"),
            Ok(HermesPermissionMode::Yolo)
        );
        assert!(HermesPermissionMode::parse("other").is_err());
    }

    #[test]
    fn codex_permission_relationships_reject_adapter_violations() {
        let hostile = EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::Always,
            FilesystemAccess::Workspace,
            NetworkAccess::Enabled,
            WebSearchAccess::Disabled,
        );
        assert!(
            CodexSelection::new(test_profile(), None, hostile, None, None, None).is_err(),
            "codex rejects always approval"
        );
        let offline_host = EnginePermissionPolicy::new(
            PermissionId::parse("permission-test").expect("permission id is valid"),
            EngineAgentId::parse("agent-test").expect("agent id is valid"),
            ApprovalMode::Never,
            FilesystemAccess::Host,
            NetworkAccess::Disabled,
            WebSearchAccess::Disabled,
        );
        assert!(
            CodexSelection::new(test_profile(), None, offline_host, None, None, None).is_err(),
            "codex rejects host scope without network"
        );
        assert!(CodexModelContextWindow::new(0).is_err());
        assert!(CodexModelContextWindow::new(1).is_ok());
        assert!(CodexReasoningEffort::parse("ultra").is_ok());
        assert!(CodexReasoningEffort::parse("turbo").is_err());
        assert!(CodexServiceTier::parse("fast").is_ok());
        assert!(CodexServiceTier::parse("priority").is_err());
    }

    #[test]
    fn claude_selection_requires_write_and_network_access() {
        assert!(
            ClaudeSelection::new(
                test_profile(),
                None,
                test_permission(),
                None,
                None,
                false,
                false,
            )
            .is_err(),
            "claude rejects read-only policy"
        );
        assert!(
            ClaudeSelection::new(
                test_profile(),
                Some(test_model()),
                writable_permission(),
                Some(ClaudeEffort::High),
                Some(ClaudePermissionMode::Plan),
                true,
                false,
            )
            .is_ok()
        );
        assert!(ClaudeEffort::parse("max").is_ok());
        assert!(ClaudeEffort::parse("ultra").is_err());
        assert!(ClaudePermissionMode::parse("acceptEdits").is_ok());
        assert!(ClaudePermissionMode::parse("yes").is_err());
    }

    #[test]
    fn open_ended_effort_labels_reject_empty_and_unbounded_values() {
        assert!(GrokReasoningEffort::parse("medium").is_ok());
        assert!(GrokReasoningEffort::parse("").is_err());
        assert!(GrokReasoningEffort::parse("has space").is_err());
        assert!(CursorReasoningEffort::parse("high").is_ok());
        assert!(CursorReasoningEffort::parse("").is_err());
        assert!(HermesReasoningEffort::parse("medium").is_ok());
        assert!(HermesReasoningEffort::parse("").is_err());
        assert_eq!(
            GrokPermissionMode::parse("always-approve"),
            Ok(GrokPermissionMode::AlwaysApprove)
        );
        assert!(GrokPermissionMode::parse("yolo").is_err());
        assert_eq!(
            CursorPermissionMode::parse("force"),
            Ok(CursorPermissionMode::Force)
        );
        assert!(CursorPermissionMode::parse("ask").is_err());
        assert_eq!(CursorSpeed::parse("fast"), Ok(CursorSpeed::Fast));
        assert!(CursorSpeed::parse("slow").is_err());
    }

    #[test]
    fn storage_codec_version_is_legacy_only_for_opencode2() {
        let legacy = complete_config("default");
        assert_eq!(legacy.engine_id(), EngineId::OpenCode2);
        assert_eq!(legacy.storage_codec_version(), 1);
        let codex = EngineRunConfig::new(
            EngineSelection::Codex(
                CodexSelection::new(test_profile(), None, test_permission(), None, None, None)
                    .expect("codex selection is valid"),
            ),
            runtime(5).expect("runtime is valid"),
        );
        assert_eq!(codex.engine_id(), EngineId::Codex);
        assert_eq!(codex.storage_codec_version(), 2);
    }
}
