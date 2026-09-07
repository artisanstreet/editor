//! Dependency-free model-selector presentation policy.
//!
//! This is the pure Rust counterpart of
//! `routes/components/model-selector/presentation.ts`.  The surrounding
//! catalog, settings store, protocol, and UI adapters own their full shapes;
//! this module keeps only the fields read by the five deterministic helpers.
//! In particular, it does not validate, persist, or mutate a session.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// A model's stable engine identifier and catalog identifier are intentionally
/// open strings, just as the protocol's identifiers are.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelChoice {
    /// Harness/engine owning this model.
    pub engine: String,
    /// Stable catalog identifier for this model.
    pub id: String,
    /// The subset of the model definition used by this presentation leaf.
    pub definition: ModelDefinition,
}

impl ModelChoice {
    /// Creates a model row from its engine, id, and selected definition fields.
    #[must_use]
    pub fn new(
        engine: impl Into<String>,
        id: impl Into<String>,
        definition: ModelDefinition,
    ) -> Self {
        Self {
            engine: engine.into(),
            id: id.into(),
            definition,
        }
    }
}

/// The disabled marker read by the selector.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DisabledState {
    /// Provider/catalog explanation for why the model is disabled.
    pub reason: String,
}

impl DisabledState {
    /// Creates a disabled marker without interpreting its reason.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

/// Presentation-level thinking choices.  The persisted `low` value is not a
/// selector choice and is represented separately by [`ReasoningEffort`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThinkingLevel {
    /// The selector's light effort.
    Light,
    /// The selector's medium effort.
    Medium,
    /// The selector's high effort.
    High,
    /// The selector's extra-high effort.
    XHigh,
    /// The selector's maximum effort.
    Max,
    /// The selector's intentionally special ultra effort.
    Ultra,
}

/// Persisted session effort values accepted by the protocol.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReasoningEffort {
    /// The legacy persisted spelling that presents as [`ThinkingLevel::Light`].
    Low,
    /// Medium effort.
    Medium,
    /// High effort.
    High,
    /// Extra-high effort.
    XHigh,
    /// Maximum effort.
    Max,
    /// Ultra effort.
    Ultra,
}

impl ReasoningEffort {
    /// Converts a saved effort to the selector's thinking vocabulary.
    #[must_use]
    pub const fn presentation_level(self) -> ThinkingLevel {
        match self {
            Self::Low => ThinkingLevel::Light,
            Self::Medium => ThinkingLevel::Medium,
            Self::High => ThinkingLevel::High,
            Self::XHigh => ThinkingLevel::XHigh,
            Self::Max => ThinkingLevel::Max,
            Self::Ultra => ThinkingLevel::Ultra,
        }
    }
}

/// The only capability facts needed by the legacy thinking resolver.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThinkingCapability {
    /// Thinking is supported and has a curated selector default.
    Supported { default: ThinkingLevel },
    /// Thinking is exposed natively and is not represented by this selector.
    Native,
    /// Thinking is unavailable.
    Unavailable,
}

/// A context-window option, including the native suffix used to select it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ContextWindowChoice {
    /// Stable catalog option id.
    pub id: String,
    /// Exact suffix appended by a native harness for this option.
    pub native_suffix: String,
}

impl ContextWindowChoice {
    /// Creates a context option without normalizing either identifier.
    #[must_use]
    pub fn new(id: impl Into<String>, native_suffix: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            native_suffix: native_suffix.into(),
        }
    }
}

/// The configurable context-window capability of one model.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ContextWindowCapability {
    /// Declared option id used when no saved suffix resolves.
    pub default: String,
    /// Options in catalog presentation order.
    pub options: Vec<ContextWindowChoice>,
}

impl ContextWindowCapability {
    /// Creates a context capability while retaining option order verbatim.
    #[must_use]
    pub fn new(default: impl Into<String>, options: Vec<ContextWindowChoice>) -> Self {
        Self {
            default: default.into(),
            options,
        }
    }
}

/// The model-definition fields consumed by this leaf.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelDefinition {
    /// Present for every disabled model, regardless of the reason text.
    pub disabled: Option<DisabledState>,
    /// Thinking and context capabilities used by default projection.
    pub capabilities: ModelCapabilities,
}

impl ModelDefinition {
    /// Creates an enabled model definition from the relevant capabilities.
    #[must_use]
    pub fn new(
        thinking: ThinkingCapability,
        context_window: Option<ContextWindowCapability>,
    ) -> Self {
        Self {
            disabled: None,
            capabilities: ModelCapabilities {
                thinking,
                context_window,
            },
        }
    }

    /// Creates a disabled model definition from the relevant capabilities.
    #[must_use]
    pub fn disabled(
        reason: impl Into<String>,
        thinking: ThinkingCapability,
        context_window: Option<ContextWindowCapability>,
    ) -> Self {
        Self {
            disabled: Some(DisabledState::new(reason)),
            capabilities: ModelCapabilities {
                thinking,
                context_window,
            },
        }
    }
}

/// The nested capability fields read from a model definition.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelCapabilities {
    /// Thinking availability and supported default.
    pub thinking: ThinkingCapability,
    /// Optional context-window capability.
    pub context_window: Option<ContextWindowCapability>,
}

/// One saved per-model defaults row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SavedModelDefaults {
    /// Model id to which these overrides belong.
    pub model_id: String,
    /// Saved protocol effort, if the user has selected one.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Saved native context suffix, if the user has selected one.
    pub context_window: Option<String>,
}

impl SavedModelDefaults {
    /// Creates a saved defaults row without applying it to another model.
    #[must_use]
    pub fn new(
        model_id: impl Into<String>,
        reasoning_effort: Option<ReasoningEffort>,
        context_window: Option<String>,
    ) -> Self {
        Self {
            model_id: model_id.into(),
            reasoning_effort,
            context_window,
        }
    }
}

/// The session-defaults portion read by this presentation policy.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SessionDefaults {
    /// Saved rows are searched in their input order; the first matching row
    /// has the same precedence as the legacy `find` call.
    pub models: Vec<SavedModelDefaults>,
}

impl SessionDefaults {
    /// Creates defaults with the supplied saved model rows.
    #[must_use]
    pub fn new(models: Vec<SavedModelDefaults>) -> Self {
        Self { models }
    }
}

/// The shared permission scale exposed by the catalog.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PermissionLevel {
    /// Read-only access.
    Restricted,
    /// Ordinary automatic access.
    Autonomous,
    /// Unrestricted access.
    Unrestricted,
}

/// A permission option.  The presentation helper returns this value
/// unchanged; fields unrelated to that operation are intentionally omitted.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct PermissionOption {
    /// Stable permission option id.
    pub id: PermissionLevel,
}

impl PermissionOption {
    /// Creates one permission option.
    #[must_use]
    pub const fn new(id: PermissionLevel) -> Self {
        Self { id }
    }
}

/// The compatibility permission mode carried by a session policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PermissionMode {
    /// Never prompt for permission.
    Never,
    /// Ask when an operation needs approval.
    OnRequest,
}

/// The compatibility sandbox mode carried by a session policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SandboxMode {
    /// No writes are allowed.
    ReadOnly,
    /// Writes are confined to the workspace.
    WorkspaceWrite,
}

/// The multi-axis session-policy subset that this selector can patch.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SessionPolicy {
    /// Selected engine identifier.
    pub engine_id: String,
    /// Selected model identifier, when the policy names one.
    pub model_id: Option<String>,
    /// Saved native context suffix, when one is selected.
    pub context_window: Option<String>,
    /// Persisted session reasoning effort.
    pub reasoning_effort: ReasoningEffort,
    /// Shared permission choice.
    pub permission: PermissionLevel,
    /// Coarse permission compatibility axis.
    pub permission_mode: PermissionMode,
    /// Coarse sandbox compatibility axis.
    pub sandbox_mode: SandboxMode,
    /// Whether web search is enabled for the session.
    pub web_search_enabled: bool,
    /// Whether strict clarification is enabled for the session.
    pub strict_clarification: bool,
}

/// Fields supplied by a partial session-policy patch.
///
/// `None` means that the axis was not supplied and therefore remains intact.
/// The legacy typed partial patch has no explicit clear value for optional
/// fields; this helper consequently only applies present values.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SessionPolicyPatch {
    /// Optional replacement engine id.
    pub engine_id: Option<String>,
    /// Optional replacement model id.
    pub model_id: Option<String>,
    /// Optional replacement native context suffix.
    pub context_window: Option<String>,
    /// Optional replacement reasoning effort.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Optional replacement shared permission.
    pub permission: Option<PermissionLevel>,
    /// Optional replacement permission mode.
    pub permission_mode: Option<PermissionMode>,
    /// Optional replacement sandbox mode.
    pub sandbox_mode: Option<SandboxMode>,
    /// Optional replacement web-search flag.
    pub web_search_enabled: Option<bool>,
    /// Optional replacement strict-clarification flag.
    pub strict_clarification: Option<bool>,
}

/// Applies only supplied policy axes, retaining every prior axis otherwise.
#[must_use]
pub fn apply_policy_patch(policy: &SessionPolicy, patch: &SessionPolicyPatch) -> SessionPolicy {
    let mut next = policy.clone();

    if let Some(value) = patch.engine_id.as_ref() {
        next.engine_id.clone_from(value);
    }
    if let Some(value) = patch.model_id.as_ref() {
        next.model_id = Some(value.clone());
    }
    if let Some(value) = patch.context_window.as_ref() {
        next.context_window = Some(value.clone());
    }
    if let Some(value) = patch.reasoning_effort {
        next.reasoning_effort = value;
    }
    if let Some(value) = patch.permission {
        next.permission = value;
    }
    if let Some(value) = patch.permission_mode {
        next.permission_mode = value;
    }
    if let Some(value) = patch.sandbox_mode {
        next.sandbox_mode = value;
    }
    if let Some(value) = patch.web_search_enabled {
        next.web_search_enabled = value;
    }
    if let Some(value) = patch.strict_clarification {
        next.strict_clarification = value;
    }

    next
}

/// Returns matching models in their original order, without deduplicating.
#[must_use]
pub fn models_for_engine(models: &[ModelChoice], engine: &str) -> Vec<ModelChoice> {
    models
        .iter()
        .filter(|model| model.engine == engine)
        .cloned()
        .collect()
}

/// Returns every permission for an enabled model, or no permissions for a
/// disabled model.  Permission order and duplicates are retained verbatim.
#[must_use]
pub fn permissions_for_selection(
    model: &ModelChoice,
    permissions: &[PermissionOption],
) -> Vec<PermissionOption> {
    if model.definition.disabled.is_some() {
        Vec::new()
    } else {
        permissions.to_vec()
    }
}

/// Resolves the thinking level saved for a model.
///
/// Unsupported and native capabilities return `None`.  For supported
/// thinking, the first saved row for the model wins; persisted `low` maps to
/// `light`, every other saved value is returned in the selector vocabulary,
/// and an absent row falls back to the capability default.
#[must_use]
pub fn thinking_for_defaults(
    defaults: &SessionDefaults,
    model: &ModelChoice,
) -> Option<ThinkingLevel> {
    let default = match model.definition.capabilities.thinking {
        ThinkingCapability::Supported { default } => default,
        ThinkingCapability::Native | ThinkingCapability::Unavailable => return None,
    };

    defaults
        .models
        .iter()
        .find(|entry| entry.model_id == model.id)
        .and_then(|entry| entry.reasoning_effort)
        .map(ReasoningEffort::presentation_level)
        .or(Some(default))
}

/// Resolves a model's context option by saved suffix, declared default id,
/// then first option, matching the legacy nullish fallback chain.
#[must_use]
pub fn context_for_defaults(
    defaults: &SessionDefaults,
    model: &ModelChoice,
) -> Option<ContextWindowChoice> {
    let context = model.definition.capabilities.context_window.as_ref()?;
    let saved_suffix = defaults
        .models
        .iter()
        .find(|entry| entry.model_id == model.id)
        .and_then(|entry| entry.context_window.as_deref());

    saved_suffix
        .and_then(|suffix| {
            context
                .options
                .iter()
                .find(|option| option.native_suffix == suffix)
        })
        .cloned()
        .or_else(|| {
            context
                .options
                .iter()
                .find(|option| option.id == context.default)
                .cloned()
        })
        .or_else(|| context.options.first().cloned())
}
