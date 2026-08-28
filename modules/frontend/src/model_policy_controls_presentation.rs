//! Dependency-free presentation policy for the model selector's policy controls.
//!
//! This module is the native counterpart of the derived values and selection
//! guards in `routes/components/model-selector/policy-controls.svelte`. It
//! deliberately stops at a renderer-neutral projection: an adapter owns
//! catalog decoding, persistence, callbacks, and every UI concern. Inputs and
//! outputs borrow the adapter's values, so this boundary preserves text,
//! identifiers, duplicates, and source ordering without normalizing them.

#![allow(clippy::module_name_repetitions)]

/// The two thinking states relevant to the model-selector policy.
///
/// A model with unsupported thinking has no policy options at this boundary.
/// The supported form intentionally permits arbitrary option identifiers and
/// an arbitrary default because this module projects supplied data rather than
/// validating a catalog.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThinkingCapability<'a> {
    /// Thinking is unavailable for this model.
    Unsupported,
    /// Thinking options and the capability's default are available.
    Supported {
        /// The exact capability default identifier.
        default: &'a str,
        /// Thinking options in their source order.
        options: &'a [ThinkingOption<'a>],
    },
}

impl<'a> ThinkingCapability<'a> {
    /// Creates an unsupported thinking capability.
    #[must_use]
    pub const fn unsupported() -> Self {
        Self::Unsupported
    }

    /// Creates a supported thinking capability without validating its inputs.
    #[must_use]
    pub const fn supported(default: &'a str, options: &'a [ThinkingOption<'a>]) -> Self {
        Self::Supported { default, options }
    }

    /// Returns whether this capability exposes thinking controls.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::Supported { .. })
    }
}

/// One renderer-neutral thinking option.
///
/// `label`, `description`, and `advisory` are supplied presentation text; the
/// policy never derives, trims, or otherwise normalizes any of them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThinkingOption<'a> {
    /// The exact selectable identifier.
    pub id: &'a str,
    /// The exact display label.
    pub label: &'a str,
    /// The exact presentation group used for partitioning.
    pub presentation_group: &'a str,
    /// Optional exact descriptive text.
    pub description: Option<&'a str>,
    /// Optional exact advisory text.
    pub advisory: Option<&'a str>,
}

impl<'a> ThinkingOption<'a> {
    /// Creates a thinking option from exact borrowed presentation values.
    #[must_use]
    pub const fn new(
        id: &'a str,
        label: &'a str,
        presentation_group: &'a str,
        description: Option<&'a str>,
        advisory: Option<&'a str>,
    ) -> Self {
        Self {
            id,
            label,
            presentation_group,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpeedOption<'a> {
    /// The exact selectable identifier.
    pub id: &'a str,
    /// The exact display label.
    pub label: &'a str,
    /// The exact descriptive text.
    pub description: &'a str,
    /// Optional exact advisory text.
    pub advisory: Option<&'a str>,
    /// Whether this option is the capability's default.
    pub default: bool,
    /// The source optional disabled property.
    pub disabled: Option<bool>,
}

impl<'a> SpeedOption<'a> {
    /// Creates a speed option from exact borrowed presentation values.
    #[must_use]
    pub const fn new(
        id: &'a str,
        label: &'a str,
        description: &'a str,
        advisory: Option<&'a str>,
        default: bool,
        disabled: Option<bool>,
    ) -> Self {
        Self {
            id,
            label,
            description,
            advisory,
            default,
            disabled,
        }
    }
}

/// One configurable context-window option.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextWindowOption<'a> {
    /// The exact selectable identifier.
    pub id: &'a str,
    /// The exact display label.
    pub label: &'a str,
    /// The exact descriptive text.
    pub description: &'a str,
    /// Optional exact advisory text.
    pub advisory: Option<&'a str>,
    /// The exact native suffix used to select this option.
    pub native_suffix: &'a str,
}

impl<'a> ContextWindowOption<'a> {
    /// Creates a context option from exact borrowed presentation values.
    #[must_use]
    pub const fn new(
        id: &'a str,
        label: &'a str,
        description: &'a str,
        advisory: Option<&'a str>,
        native_suffix: &'a str,
    ) -> Self {
        Self {
            id,
            label,
            description,
            advisory,
            native_suffix,
        }
    }
}

/// One permission option supplied by the harness capability.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PermissionOption<'a> {
    /// The exact selectable identifier.
    pub id: &'a str,
    /// The exact display label.
    pub label: &'a str,
    /// The exact descriptive text.
    pub description: &'a str,
    /// Optional exact advisory text.
    pub advisory: Option<&'a str>,
}

impl<'a> PermissionOption<'a> {
    /// Creates a permission option from exact borrowed presentation values.
    #[must_use]
    pub const fn new(
        id: &'a str,
        label: &'a str,
        description: &'a str,
        advisory: Option<&'a str>,
    ) -> Self {
        Self {
            id,
            label,
            description,
            advisory,
        }
    }
}

/// The optional context-window capability of one model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextWindowCapability<'a> {
    /// The exact capability default option identifier.
    pub default: &'a str,
    /// Context options in their source order.
    pub options: &'a [ContextWindowOption<'a>],
}

impl<'a> ContextWindowCapability<'a> {
    /// Creates a context capability without validating its inputs.
    #[must_use]
    pub const fn new(default: &'a str, options: &'a [ContextWindowOption<'a>]) -> Self {
        Self { default, options }
    }
}

/// Capabilities read by the model policy-controls component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelCapabilities<'a> {
    /// Thinking support, default, and options.
    pub thinking: ThinkingCapability<'a>,
    /// All source speed options, including options with a defined disabled property.
    pub speed_options: &'a [SpeedOption<'a>],
    /// The optional configurable context-window capability.
    pub context_window: Option<ContextWindowCapability<'a>>,
}

impl<'a> ModelCapabilities<'a> {
    /// Creates the policy-relevant capabilities for one model.
    #[must_use]
    pub const fn new(
        thinking: ThinkingCapability<'a>,
        speed_options: &'a [SpeedOption<'a>],
        context_window: Option<ContextWindowCapability<'a>>,
    ) -> Self {
        Self {
            thinking,
            speed_options,
            context_window,
        }
    }
}

/// A compact model choice with the capabilities needed by this policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Model<'a> {
    /// The exact model identifier used for selected-model comparison.
    pub id: &'a str,
    /// The exact model label retained for the eventual renderer.
    pub label: &'a str,
    /// The model's policy-relevant capabilities.
    pub capabilities: ModelCapabilities<'a>,
}

impl<'a> Model<'a> {
    /// Creates a model choice from exact borrowed values.
    #[must_use]
    pub const fn new(id: &'a str, label: &'a str, capabilities: ModelCapabilities<'a>) -> Self {
        Self {
            id,
            label,
            capabilities,
        }
    }
}

/// The policy field consulted for selected-model context suffix matching.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelPolicy<'a> {
    /// An optional exact native context suffix.
    pub context_window: Option<&'a str>,
}

impl<'a> ModelPolicy<'a> {
    /// Creates a model policy from an optional exact context suffix.
    #[must_use]
    pub const fn new(context_window: Option<&'a str>) -> Self {
        Self { context_window }
    }
}

/// Inputs already selected or derived by the surrounding model-selector layer.
///
/// The fields correspond only to the values read by the source component.
/// Rendering-disabled state, effects, callbacks, persistence, transport, and
/// catalog registration are intentionally outside this boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelPolicyControlsInput<'a> {
    /// The model whose controls are being projected.
    pub model: Model<'a>,
    /// The exact id of the globally selected model.
    pub selected_model_id: &'a str,
    /// The caller's current thinking level.
    pub thinking_level: &'a str,
    /// The caller's current speed option id.
    pub speed_option_id: &'a str,
    /// The live model policy, if one has been supplied.
    pub policy: Option<ModelPolicy<'a>>,
    /// The live permission mode used for first permission matching.
    pub permission_mode: &'a str,
    /// The optional permission default id used after the live mode.
    pub permission_default: Option<&'a str>,
    /// Variant model choices in their source order.
    pub variant_options: &'a [Model<'a>],
    /// Permission choices in their source order.
    pub permission_options: &'a [PermissionOption<'a>],
}

/// The renderer-neutral projection of the policy controls.
///
/// Candidate vectors are copied as small borrowed records so filtered and
/// partitioned lists remain explicit while every string still points at the
/// supplied input. A present empty `context_options` value means the capability
/// exists but has no options; `None` means that the capability is absent.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelPolicyControlsPresentation<'a> {
    /// The model whose controls were projected.
    pub model: Model<'a>,
    /// Variant candidates, preserving source order and duplicates.
    pub variant_options: Vec<Model<'a>>,
    /// Supported thinking options in the exact `base` group and source order.
    pub base_thinking_options: Vec<ThinkingOption<'a>>,
    /// Supported thinking options in the exact `special` group and source order.
    pub special_thinking_options: Vec<ThinkingOption<'a>>,
    /// Enabled speed candidates in source order.
    pub speed_options: Vec<SpeedOption<'a>>,
    /// Context candidates when the capability is present, otherwise `None`.
    pub context_options: Option<Vec<ContextWindowOption<'a>>>,
    /// Permission candidates in source order.
    pub permission_options: Vec<PermissionOption<'a>>,
    /// The selected-model level or non-selected model default, when supported.
    pub current_thinking: Option<&'a str>,
    /// The selected speed candidate after the source precedence chain.
    pub current_speed: Option<SpeedOption<'a>>,
    /// The selected context candidate after the source precedence chain.
    pub current_context: Option<ContextWindowOption<'a>>,
    /// The selected permission candidate after the source precedence chain.
    pub current_permission: Option<PermissionOption<'a>>,
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelPolicySelection<'a> {
    /// Attempt to select a thinking option by exact id.
    Thinking { id: &'a str },
    /// Attempt to select a speed option by exact id.
    Speed { id: &'a str },
    /// Attempt to select a context option by exact id.
    Context { id: &'a str },
    /// Attempt to select a permission option by exact id.
    Permission { id: &'a str },
    /// Attempt to select a variant model by exact id.
    Variant { id: &'a str },
}

/// A typed action admitted by the model policy-controls boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelPolicyAction<'a> {
    /// Select the exact thinking option on the input model.
    Thinking {
        /// The model whose thinking policy changes.
        model: Model<'a>,
        /// The first exact matching thinking option.
        option: ThinkingOption<'a>,
    },
    /// Select the exact enabled speed option on the input model.
    Speed {
        /// The model whose speed policy changes.
        model: Model<'a>,
        /// The first exact matching enabled speed option.
        option: SpeedOption<'a>,
    },
    /// Select the exact context option on the input model.
    Context {
        /// The model whose context policy changes.
        model: Model<'a>,
        /// The first exact matching context option.
        option: ContextWindowOption<'a>,
    },
    /// Select the exact permission option.
    Permission {
        /// The model associated with the control row.
        model: Model<'a>,
        /// The first exact matching permission option.
        option: PermissionOption<'a>,
    },
    /// Select the exact variant model.
    Variant {
        /// The first exact matching variant model.
        model: Model<'a>,
    },
}

fn project_thinking_options<'a>(
    input: &ModelPolicyControlsInput<'a>,
) -> (
    Vec<ThinkingOption<'a>>,
    Vec<ThinkingOption<'a>>,
    Option<&'a str>,
) {
    match input.model.capabilities.thinking {
        ThinkingCapability::Unsupported => (Vec::new(), Vec::new(), None),
        ThinkingCapability::Supported { default, options } => {
            let mut base = Vec::new();
            let mut special = Vec::new();
            for option in options {
                match option.presentation_group {
                    "base" => base.push(*option),
                    "special" => special.push(*option),
                    _ => {}
                }
            }
            let current = if input.model.id == input.selected_model_id {
                Some(input.thinking_level)
            } else {
                Some(default)
            };
            (base, special, current)
        }
    }
}

fn enabled_speed_options<'a>(input: &ModelPolicyControlsInput<'a>) -> Vec<SpeedOption<'a>> {
    input
        .model
        .capabilities
        .speed_options
        .iter()
        .copied()
        .filter(|option| option.disabled.is_none())
        .collect()
}

fn current_speed<'a>(
    input: &ModelPolicyControlsInput<'a>,
    speed_options: &[SpeedOption<'a>],
) -> Option<SpeedOption<'a>> {
    let selected_speed = if input.model.id == input.selected_model_id {
        speed_options
            .iter()
            .find(|option| option.id == input.speed_option_id)
            .copied()
    } else {
        None
    };
    selected_speed
        .or_else(|| speed_options.iter().find(|option| option.default).copied())
        .or_else(|| speed_options.first().copied())
}

fn project_context<'a>(
    input: &ModelPolicyControlsInput<'a>,
) -> (
    Option<Vec<ContextWindowOption<'a>>>,
    Option<ContextWindowOption<'a>>,
) {
    match input.model.capabilities.context_window {
        None => (None, None),
        Some(capability) => {
            let suffix_match = if input.model.id == input.selected_model_id {
                input.policy.and_then(|policy| {
                    policy.context_window.and_then(|suffix| {
                        capability
                            .options
                            .iter()
                            .find(|option| option.native_suffix == suffix)
                            .copied()
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
                        .copied()
                })
                .or_else(|| capability.options.first().copied());
            (Some(capability.options.to_vec()), current)
        }
    }
}

fn current_permission<'a>(input: &ModelPolicyControlsInput<'a>) -> Option<PermissionOption<'a>> {
    input
        .permission_options
        .iter()
        .find(|option| option.id == input.permission_mode)
        .copied()
        .or_else(|| {
            input.permission_default.and_then(|default| {
                input
                    .permission_options
                    .iter()
                    .find(|option| option.id == default)
                    .copied()
            })
        })
        .or_else(|| input.permission_options.first().copied())
}

/// Projects model policy controls using the legacy selection and visibility rules.
///
/// Thinking options are partitioned only for supported capabilities. Speed
/// candidates retain only options whose `disabled` property is absent. Current
/// values use the same selected-model, suffix, default, and first-option
/// precedence as the source component. No input value is normalized.
#[must_use]
pub fn project_model_policy_controls<'a>(
    input: &ModelPolicyControlsInput<'a>,
) -> ModelPolicyControlsPresentation<'a> {
    let (base_thinking_options, special_thinking_options, current_thinking) =
        project_thinking_options(input);
    let speed_options = enabled_speed_options(input);
    let current_speed = current_speed(input, &speed_options);
    let (context_options, current_context) = project_context(input);
    let current_permission = current_permission(input);

    let variant_options = input.variant_options.to_vec();
    let permission_options = input.permission_options.to_vec();

    ModelPolicyControlsPresentation {
        model: input.model,
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
pub fn admit_model_policy_action<'a>(
    input: &ModelPolicyControlsInput<'a>,
    selection: ModelPolicySelection<'_>,
) -> Option<ModelPolicyAction<'a>> {
    match selection {
        ModelPolicySelection::Thinking { id } => {
            let ThinkingCapability::Supported { options, .. } = input.model.capabilities.thinking
            else {
                return None;
            };
            let option = options.iter().find(|option| option.id == id).copied()?;
            Some(ModelPolicyAction::Thinking {
                model: input.model,
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
                .copied()?;
            Some(ModelPolicyAction::Speed {
                model: input.model,
                option,
            })
        }
        ModelPolicySelection::Context { id } => {
            let capability = input.model.capabilities.context_window?;
            let option = capability
                .options
                .iter()
                .find(|option| option.id == id)
                .copied()?;
            Some(ModelPolicyAction::Context {
                model: input.model,
                option,
            })
        }
        ModelPolicySelection::Permission { id } => {
            let option = input
                .permission_options
                .iter()
                .find(|option| option.id == id)
                .copied()?;
            Some(ModelPolicyAction::Permission {
                model: input.model,
                option,
            })
        }
        ModelPolicySelection::Variant { id } => {
            let model = input
                .variant_options
                .iter()
                .find(|model| model.id == id)
                .copied()?;
            Some(ModelPolicyAction::Variant { model })
        }
    }
}
