//! Dependency-free presentation policy for the model selector's policy controls.
//!
//! This module is the native counterpart of the derived values and selection
//! guards in `routes/components/model-selector/policy-controls.svelte`. It
//! deliberately stops at a renderer-neutral projection: an adapter owns
//! catalog decoding, persistence, callbacks, and every UI concern. The policy
//! boundary owns its input and output records, so a projection or action can
//! outlive the decoded/catalog source rows without normalizing their text,
//! identifiers, duplicates, or source ordering.

#![allow(clippy::module_name_repetitions)]

/// The two thinking states relevant to the model-selector policy.
///
/// A model with unsupported thinking has no policy options at this boundary.
/// The supported form intentionally permits arbitrary option identifiers and
/// an arbitrary default because this module projects supplied data rather than
/// validating a catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThinkingCapability {
    /// Thinking is unavailable for this model.
    Unsupported,
    /// Thinking options and the capability's default are available.
    Supported {
        /// The exact capability default identifier.
        default: String,
        /// Thinking options in their source order.
        options: Vec<ThinkingOption>,
    },
}

impl ThinkingCapability {
    /// Creates an unsupported thinking capability.
    #[must_use]
    pub const fn unsupported() -> Self {
        Self::Unsupported
    }

    /// Creates a supported thinking capability without validating its inputs.
    #[must_use]
    pub fn supported(default: impl Into<String>, options: Vec<ThinkingOption>) -> Self {
        Self::Supported {
            default: default.into(),
            options,
        }
    }

    /// Returns whether this capability exposes thinking controls.
    #[must_use]
    pub const fn is_supported(&self) -> bool {
        matches!(self, Self::Supported { .. })
    }
}

/// One renderer-neutral thinking option.
///
/// `label`, `description`, and `advisory` are supplied presentation text; the
/// policy never derives, trims, or otherwise normalizes any of them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThinkingOption {
    /// The exact selectable identifier.
    pub id: String,
    /// The exact display label.
    pub label: String,
    /// The exact presentation group used for partitioning.
    pub presentation_group: String,
    /// Optional exact descriptive text.
    pub description: Option<String>,
    /// Optional exact advisory text.
    pub advisory: Option<String>,
}

impl ThinkingOption {
    /// Creates a thinking option from exact presentation values.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        presentation_group: impl Into<String>,
        description: Option<String>,
        advisory: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            presentation_group: presentation_group.into(),
            description,
            advisory,
        }
    }
}

/// One speed candidate supplied by a model capability.
///
/// `disabled` models the source's optional property. Only `None` is an
/// enabled candidate: `Some(false)` is still a defined property and is
/// intentionally excluded by the legacy selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeedOption {
    /// The exact selectable identifier.
    pub id: String,
    /// The exact display label.
    pub label: String,
    /// The exact descriptive text.
    pub description: String,
    /// Optional exact advisory text.
    pub advisory: Option<String>,
    /// Whether this option is the capability's default.
    pub default: bool,
    /// The source optional disabled property.
    pub disabled: Option<bool>,
}

impl SpeedOption {
    /// Creates a speed option from exact presentation values.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        advisory: Option<String>,
        default: bool,
        disabled: Option<bool>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            advisory,
            default,
            disabled,
        }
    }
}

/// One configurable context-window option.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextWindowOption {
    /// The exact selectable identifier.
    pub id: String,
    /// The exact display label.
    pub label: String,
    /// The exact descriptive text.
    pub description: String,
    /// Optional exact advisory text.
    pub advisory: Option<String>,
    /// The exact native suffix used to select this option.
    pub native_suffix: String,
}

impl ContextWindowOption {
    /// Creates a context option from exact presentation values.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        advisory: Option<String>,
        native_suffix: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            advisory,
            native_suffix: native_suffix.into(),
        }
    }
}

/// One permission option supplied by the harness capability.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionOption {
    /// The exact selectable identifier.
    pub id: String,
    /// The exact display label.
    pub label: String,
    /// The exact descriptive text.
    pub description: String,
    /// Optional exact advisory text.
    pub advisory: Option<String>,
}

impl PermissionOption {
    /// Creates a permission option from exact presentation values.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
        advisory: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: description.into(),
            advisory,
        }
    }
}

/// The optional context-window capability of one model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextWindowCapability {
    /// The exact capability default option identifier.
    pub default: String,
    /// Context options in their source order.
    pub options: Vec<ContextWindowOption>,
}

impl ContextWindowCapability {
    /// Creates a context capability without validating its inputs.
    #[must_use]
    pub fn new(default: impl Into<String>, options: Vec<ContextWindowOption>) -> Self {
        Self {
            default: default.into(),
            options,
        }
    }
}

/// Capabilities read by the model policy-controls component.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelCapabilities {
    /// Thinking support, default, and options.
    pub thinking: ThinkingCapability,
    /// All source speed options, including options with a defined disabled property.
    pub speed_options: Vec<SpeedOption>,
    /// The optional configurable context-window capability.
    pub context_window: Option<ContextWindowCapability>,
}

impl ModelCapabilities {
    /// Creates the policy-relevant capabilities for one model.
    #[must_use]
    pub fn new(
        thinking: ThinkingCapability,
        speed_options: Vec<SpeedOption>,
        context_window: Option<ContextWindowCapability>,
    ) -> Self {
        Self {
            thinking,
            speed_options,
            context_window,
        }
    }
}

/// A compact model choice with the capabilities needed by this policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Model {
    /// The exact model identifier used for selected-model comparison.
    pub id: String,
    /// The exact model label retained for the eventual renderer.
    pub label: String,
    /// The model's policy-relevant capabilities.
    pub capabilities: ModelCapabilities,
}

impl Model {
    /// Creates a model choice from exact values.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        capabilities: ModelCapabilities,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            capabilities,
        }
    }
}

/// The policy field consulted for selected-model context suffix matching.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelPolicy {
    /// An optional exact native context suffix.
    pub context_window: Option<String>,
}

impl ModelPolicy {
    /// Creates a model policy from an optional exact context suffix.
    #[must_use]
    pub fn new(context_window: Option<String>) -> Self {
        Self { context_window }
    }
}

/// Inputs already selected or derived by the surrounding model-selector layer.
///
/// The fields correspond only to the values read by the source component.
/// Rendering-disabled state, effects, callbacks, persistence, transport, and
/// catalog registration are intentionally outside this boundary. Every field
/// owns its text and collections so callers may drop their source rows after
/// constructing this value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelPolicyControlsInput {
    /// The model whose controls are being projected.
    pub model: Model,
    /// The exact id of the globally selected model.
    pub selected_model_id: String,
    /// The caller's current thinking level.
    pub thinking_level: String,
    /// The caller's current speed option id.
    pub speed_option_id: String,
    /// The live model policy, if one has been supplied.
    pub policy: Option<ModelPolicy>,
    /// The live permission mode used for first permission matching.
    pub permission_mode: String,
    /// The optional permission default id used after the live mode.
    pub permission_default: Option<String>,
    /// Variant model choices in their source order.
    pub variant_options: Vec<Model>,
    /// Permission choices in their source order.
    pub permission_options: Vec<PermissionOption>,
}

impl ModelPolicyControlsInput {
    /// Creates an owned set of policy-control inputs.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        model: Model,
        selected_model_id: impl Into<String>,
        thinking_level: impl Into<String>,
        speed_option_id: impl Into<String>,
        policy: Option<ModelPolicy>,
        permission_mode: impl Into<String>,
        permission_default: Option<String>,
        variant_options: Vec<Model>,
        permission_options: Vec<PermissionOption>,
    ) -> Self {
        Self {
            model,
            selected_model_id: selected_model_id.into(),
            thinking_level: thinking_level.into(),
            speed_option_id: speed_option_id.into(),
            policy,
            permission_mode: permission_mode.into(),
            permission_default,
            variant_options,
            permission_options,
        }
    }
}

/// The renderer-neutral projection of the policy controls.
///
/// Candidate vectors are owned copies so filtered and partitioned lists remain
/// explicit while the projection remains usable after the input is dropped. A
/// present empty `context_options` value means the capability exists but has
/// no options; `None` means that the capability is absent.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelPolicyControlsPresentation {
    /// The model whose controls were projected.
    pub model: Model,
    /// Variant candidates, preserving source order and duplicates.
    pub variant_options: Vec<Model>,
    /// Supported thinking options in the exact `base` group and source order.
    pub base_thinking_options: Vec<ThinkingOption>,
    /// Supported thinking options in the exact `special` group and source order.
    pub special_thinking_options: Vec<ThinkingOption>,
    /// Enabled speed candidates in source order.
    pub speed_options: Vec<SpeedOption>,
    /// Context candidates when the capability is present, otherwise `None`.
    pub context_options: Option<Vec<ContextWindowOption>>,
    /// Permission candidates in source order.
    pub permission_options: Vec<PermissionOption>,
    /// The selected-model level or non-selected model default, when supported.
    pub current_thinking: Option<String>,
    /// The selected speed candidate after the source precedence chain.
    pub current_speed: Option<SpeedOption>,
    /// The selected context candidate after the source precedence chain.
    pub current_context: Option<ContextWindowOption>,
    /// The selected permission candidate after the source precedence chain.
    pub current_permission: Option<PermissionOption>,
    /// Whether the variant control is visible.
    pub show_variant: bool,
    /// Whether the thinking control is visible.
    pub show_thinking: bool,
    /// Whether the speed control is visible.
    pub show_speed: bool,
    /// Whether the context control is visible.
    pub show_context: bool,
    /// Whether the permission control is visible.
    pub show_permission: bool,
}

/// A renderer-neutral attempted selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelPolicySelection {
    /// Attempt to select a thinking option by exact id.
    Thinking {
        /// The exact requested option identifier.
        id: String,
    },
    /// Attempt to select a speed option by exact id.
    Speed {
        /// The exact requested option identifier.
        id: String,
    },
    /// Attempt to select a context option by exact id.
    Context {
        /// The exact requested option identifier.
        id: String,
    },
    /// Attempt to select a permission option by exact id.
    Permission {
        /// The exact requested option identifier.
        id: String,
    },
    /// Attempt to select a variant model by exact id.
    Variant {
        /// The exact requested model identifier.
        id: String,
    },
}

impl ModelPolicySelection {
    /// Creates a thinking selection request.
    #[must_use]
    pub fn thinking(id: impl Into<String>) -> Self {
        Self::Thinking { id: id.into() }
    }

    /// Creates a speed selection request.
    #[must_use]
    pub fn speed(id: impl Into<String>) -> Self {
        Self::Speed { id: id.into() }
    }

    /// Creates a context selection request.
    #[must_use]
    pub fn context(id: impl Into<String>) -> Self {
        Self::Context { id: id.into() }
    }

    /// Creates a permission selection request.
    #[must_use]
    pub fn permission(id: impl Into<String>) -> Self {
        Self::Permission { id: id.into() }
    }

    /// Creates a variant selection request.
    #[must_use]
    pub fn variant(id: impl Into<String>) -> Self {
        Self::Variant { id: id.into() }
    }
}

/// A typed action admitted by the model policy-controls boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelPolicyAction {
    /// Select the exact thinking option on the input model.
    Thinking {
        /// The model whose thinking policy changes.
        model: Model,
        /// The first exact matching thinking option.
        option: ThinkingOption,
    },
    /// Select the exact enabled speed option on the input model.
    Speed {
        /// The model whose speed policy changes.
        model: Model,
        /// The first exact matching enabled speed option.
        option: SpeedOption,
    },
    /// Select the exact context option on the input model.
    Context {
        /// The model whose context policy changes.
        model: Model,
        /// The first exact matching context option.
        option: ContextWindowOption,
    },
    /// Select the exact permission option.
    Permission {
        /// The model associated with the control row.
        model: Model,
        /// The first exact matching permission option.
        option: PermissionOption,
    },
    /// Select the exact variant model.
    Variant {
        /// The first exact matching variant model.
        model: Model,
    },
}

fn project_thinking_options(
    input: &ModelPolicyControlsInput,
) -> (Vec<ThinkingOption>, Vec<ThinkingOption>, Option<String>) {
    match &input.model.capabilities.thinking {
        ThinkingCapability::Unsupported => (Vec::new(), Vec::new(), None),
        ThinkingCapability::Supported { default, options } => {
            let mut base = Vec::new();
            let mut special = Vec::new();
            for option in options {
                match option.presentation_group.as_str() {
                    "base" => base.push(option.clone()),
                    "special" => special.push(option.clone()),
                    _ => {}
                }
            }
            let current = if input.model.id == input.selected_model_id {
                Some(input.thinking_level.clone())
            } else {
                Some(default.clone())
            };
            (base, special, current)
        }
    }
}

fn enabled_speed_options(input: &ModelPolicyControlsInput) -> Vec<SpeedOption> {
    input
        .model
        .capabilities
        .speed_options
        .iter()
        .filter(|option| option.disabled.is_none())
        .cloned()
        .collect()
}

fn current_speed(
    input: &ModelPolicyControlsInput,
    speed_options: &[SpeedOption],
) -> Option<SpeedOption> {
    let selected_speed = if input.model.id == input.selected_model_id {
        speed_options
            .iter()
            .find(|option| option.id == input.speed_option_id)
            .cloned()
    } else {
        None
    };
    selected_speed
        .or_else(|| speed_options.iter().find(|option| option.default).cloned())
        .or_else(|| speed_options.first().cloned())
}

fn project_context(
    input: &ModelPolicyControlsInput,
) -> (
    Option<Vec<ContextWindowOption>>,
    Option<ContextWindowOption>,
) {
    match input.model.capabilities.context_window.as_ref() {
        None => (None, None),
        Some(capability) => {
            let suffix_match = if input.model.id == input.selected_model_id {
                input.policy.as_ref().and_then(|policy| {
                    policy.context_window.as_ref().and_then(|suffix| {
                        capability
                            .options
                            .iter()
                            .find(|option| option.native_suffix == *suffix)
                            .cloned()
                    })
                })
            } else {
                None
            };
            let current = suffix_match
                .or_else(|| {
                    capability
                        .options
                        .iter()
                        .find(|option| option.id == capability.default)
                        .cloned()
                })
                .or_else(|| capability.options.first().cloned());
            (Some(capability.options.clone()), current)
        }
    }
}

fn current_permission(input: &ModelPolicyControlsInput) -> Option<PermissionOption> {
    input
        .permission_options
        .iter()
        .find(|option| option.id == input.permission_mode)
        .cloned()
        .or_else(|| {
            input.permission_default.as_ref().and_then(|default| {
                input
                    .permission_options
                    .iter()
                    .find(|option| option.id == *default)
                    .cloned()
            })
        })
        .or_else(|| input.permission_options.first().cloned())
}

/// Projects model policy controls using the legacy selection and visibility rules.
///
/// Thinking options are partitioned only for supported capabilities. Speed
/// candidates retain only options whose `disabled` property is absent. Current
/// values use the same selected-model, suffix, default, and first-option
/// precedence as the source component. No input value is normalized.
#[must_use]
pub fn project_model_policy_controls(
    input: &ModelPolicyControlsInput,
) -> ModelPolicyControlsPresentation {
    let (base_thinking_options, special_thinking_options, current_thinking) =
        project_thinking_options(input);
    let speed_options = enabled_speed_options(input);
    let current_speed = current_speed(input, &speed_options);
    let (context_options, current_context) = project_context(input);
    let current_permission = current_permission(input);

    let variant_options = input.variant_options.clone();
    let permission_options = input.permission_options.clone();

    ModelPolicyControlsPresentation {
        model: input.model.clone(),
        show_variant: variant_options.len() > 1,
        show_thinking: current_thinking.is_some(),
        show_speed: speed_options.len() > 1,
        show_context: context_options.is_some() && current_context.is_some(),
        show_permission: permission_options.len() > 1 && current_permission.is_some(),
        variant_options,
        base_thinking_options,
        special_thinking_options,
        speed_options,
        context_options,
        permission_options,
        current_thinking,
        current_speed,
        current_context,
        current_permission,
    }
}

/// Admits one exact selection against the source candidate list.
///
/// A valid selection returns its first matching typed action, preserving
/// duplicate-id first-match behavior. Invalid selections return `None`.
/// Visibility is intentionally not an admission guard: the source handlers
/// validate candidate membership, while visibility controls whether a handler
/// can normally be reached by a renderer.
#[must_use]
pub fn admit_model_policy_action(
    input: &ModelPolicyControlsInput,
    selection: ModelPolicySelection,
) -> Option<ModelPolicyAction> {
    match selection {
        ModelPolicySelection::Thinking { id } => {
            let ThinkingCapability::Supported { options, .. } = &input.model.capabilities.thinking
            else {
                return None;
            };
            let option = options.iter().find(|option| option.id == id).cloned()?;
            Some(ModelPolicyAction::Thinking {
                model: input.model.clone(),
                option,
            })
        }
        ModelPolicySelection::Speed { id } => {
            let option = input
                .model
                .capabilities
                .speed_options
                .iter()
                .find(|option| option.disabled.is_none() && option.id == id)
                .cloned()?;
            Some(ModelPolicyAction::Speed {
                model: input.model.clone(),
                option,
            })
        }
        ModelPolicySelection::Context { id } => {
            let capability = input.model.capabilities.context_window.as_ref()?;
            let option = capability
                .options
                .iter()
                .find(|option| option.id == id)
                .cloned()?;
            Some(ModelPolicyAction::Context {
                model: input.model.clone(),
                option,
            })
        }
        ModelPolicySelection::Permission { id } => {
            let option = input
                .permission_options
                .iter()
                .find(|option| option.id == id)
                .cloned()?;
            Some(ModelPolicyAction::Permission {
                model: input.model.clone(),
                option,
            })
        }
        ModelPolicySelection::Variant { id } => {
            let model = input
                .variant_options
                .iter()
                .find(|model| model.id == id)
                .cloned()?;
            Some(ModelPolicyAction::Variant { model })
        }
    }
}
