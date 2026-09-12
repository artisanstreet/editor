use crate::identifiers::{EngineModelId, EngineProfileId, EngineRouteId};

use super::identity::{EngineConfigError, EngineConfigReason};
use super::runtime::{ApprovalMode, EnginePermissionPolicy, FilesystemAccess, NetworkAccess};
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
