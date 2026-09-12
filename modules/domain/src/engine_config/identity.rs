use thiserror::Error;

use crate::bounds::{
    ENGINE_RUNTIME_MAX_BODY_BYTES, ENGINE_RUNTIME_MAX_MILLIS, ENGINE_RUNTIME_MAX_OBSERVATIONS,
};
use crate::identifiers::{EngineModelId, EngineProfileId, EngineRouteId, EngineVariantId};

use super::providers::{ClaudeSelection, CodexSelection, CursorSelection, GrokSelection};

use super::runtime::EnginePermissionPolicy;
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
}

impl EngineId {
    /// Every stable engine identity.
    pub const ALL: [Self; 5] = [
        Self::OpenCode2,
        Self::Codex,
        Self::Claude,
        Self::Grok,
        Self::Cursor,
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
        }
    }

    /// Returns the canonical permission policy.
    #[must_use]
    pub const fn permission(&self) -> &EnginePermissionPolicy {
        match self {
            Self::OpenCode2(selection) => selection.permission(),
            Self::Codex(selection) => selection.permission(),
            Self::Claude(selection) => selection.permission(),
            Self::Grok(selection) => selection.permission(),
            Self::Cursor(selection) => selection.permission(),
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
            Self::Codex(_) | Self::Claude(_) | Self::Grok(_) | Self::Cursor(_) => Err(
                EngineConfigError::new("engine", EngineConfigReason::Unsupported),
            ),
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
            Self::OpenCode2(_) | Self::Claude(_) | Self::Grok(_) | Self::Cursor(_) => Err(
                EngineConfigError::new("engine", EngineConfigReason::Unsupported),
            ),
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
            Self::OpenCode2(_) | Self::Codex(_) | Self::Grok(_) | Self::Cursor(_) => Err(
                EngineConfigError::new("engine", EngineConfigReason::Unsupported),
            ),
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
            Self::OpenCode2(_) | Self::Codex(_) | Self::Claude(_) | Self::Cursor(_) => Err(
                EngineConfigError::new("engine", EngineConfigReason::Unsupported),
            ),
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
            Self::OpenCode2(_) | Self::Codex(_) | Self::Claude(_) | Self::Grok(_) => Err(
                EngineConfigError::new("engine", EngineConfigReason::Unsupported),
            ),
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
