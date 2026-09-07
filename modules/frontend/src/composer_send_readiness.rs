//! Pure composer send-readiness and context-window policy.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/composer/send-readiness.ts`.  The protocol
//! catalog and usage schemas are deliberately not pulled into this leaf: the
//! small borrowed input types below are the fields this policy actually
//! reads.  The eventual protocol adapter can project its wider values into
//! them without making send readiness depend on transport, GPUI, or runtime
//! state.

#![allow(clippy::module_name_repetitions)]

/// The catalog harness identity and its user-facing label.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HarnessDefinition<'a> {
    /// Stable harness/engine identifier used by a session policy.
    pub id: &'a str,
    /// Label used in the preview-only send message.
    pub label: &'a str,
}

impl<'a> HarnessDefinition<'a> {
    /// Creates a catalog harness descriptor.
    #[must_use]
    pub const fn new(id: &'a str, label: &'a str) -> Self {
        Self { id, label }
    }
}

/// The structured native selection for a route-aware catalog model.
// Keep the protocol's exact field names at this projection boundary.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NativeSelection<'a> {
    /// Provider route selected by the harness.
    pub provider_route_id: &'a str,
    /// Model identifier within the provider route.
    pub model_id: &'a str,
    /// Optional provider-owned variant within that model.
    pub variant_id: Option<&'a str>,
}

impl<'a> NativeSelection<'a> {
    /// Creates a route-aware native model selection.
    #[must_use]
    pub const fn new(
        provider_route_id: &'a str,
        model_id: &'a str,
        variant_id: Option<&'a str>,
    ) -> Self {
        Self {
            provider_route_id,
            model_id,
            variant_id,
        }
    }
}

/// One selectable context-window option in a model capability.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ContextWindowOption<'a> {
    /// Stable option identifier referenced by the capability default.
    pub id: &'a str,
    /// Native suffix appended when this option is selected.
    pub native_suffix: &'a str,
    /// Usable context-window size in tokens.
    pub tokens: u64,
}

impl<'a> ContextWindowOption<'a> {
    /// Creates a context-window option.
    #[must_use]
    pub const fn new(id: &'a str, native_suffix: &'a str, tokens: u64) -> Self {
        Self {
            id,
            native_suffix,
            tokens,
        }
    }
}

/// A model capability with a declared default and selectable options.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ContextWindowCapability<'a> {
    /// Option identifier selected when no suffix is present in the policy.
    pub default: &'a str,
    /// Options in catalog order.
    pub options: &'a [ContextWindowOption<'a>],
}

impl<'a> ContextWindowCapability<'a> {
    /// Creates a configurable context-window capability.
    #[must_use]
    pub const fn new(default: &'a str, options: &'a [ContextWindowOption<'a>]) -> Self {
        Self { default, options }
    }
}

/// The subset of model capabilities consumed by this policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModelCapabilities<'a> {
    /// Legacy provider-reported scalar context limit.
    pub context_window_tokens: Option<u64>,
    /// Configurable context-window choices, when present.
    pub context_window: Option<ContextWindowCapability<'a>>,
}

impl<'a> ModelCapabilities<'a> {
    /// Creates the context-related portion of a model capability record.
    #[must_use]
    pub const fn new(
        context_window_tokens: Option<u64>,
        context_window: Option<ContextWindowCapability<'a>>,
    ) -> Self {
        Self {
            context_window_tokens,
            context_window,
        }
    }
}

/// One model entry in the runtime catalog manifest.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModelDefinition<'a> {
    /// Harness/engine that owns this catalog model.
    pub harness: &'a str,
    /// Provider-native model identifier.
    pub native_model_id: &'a str,
    /// Route-aware native identity, when the model is scoped to a selection.
    pub native_selection: Option<NativeSelection<'a>>,
    /// Model capabilities needed by this policy.
    pub capabilities: ModelCapabilities<'a>,
}

impl<'a> ModelDefinition<'a> {
    /// Creates a minimal catalog model entry.
    #[must_use]
    pub const fn new(
        harness: &'a str,
        native_model_id: &'a str,
        native_selection: Option<NativeSelection<'a>>,
        capabilities: ModelCapabilities<'a>,
    ) -> Self {
        Self {
            harness,
            native_model_id,
            native_selection,
            capabilities,
        }
    }
}

/// The manifest portion of a runtime catalog used by this policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ModelManifest<'a> {
    /// Catalog harness descriptors, in manifest order.
    pub harnesses: &'a [HarnessDefinition<'a>],
    /// Catalog model definitions, in manifest order.
    pub models: &'a [ModelDefinition<'a>],
}

impl<'a> ModelManifest<'a> {
    /// Creates a minimal model manifest.
    #[must_use]
    pub const fn new(
        harnesses: &'a [HarnessDefinition<'a>],
        models: &'a [ModelDefinition<'a>],
    ) -> Self {
        Self { harnesses, models }
    }
}

/// The runtime catalog projection needed by composer send readiness.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RuntimeCatalog<'a> {
    /// Static model and harness manifest.
    pub manifest: ModelManifest<'a>,
    /// Harnesses that the currently connected Forge can actually run.
    pub runnable_harness_ids: &'a [&'a str],
}

impl<'a> RuntimeCatalog<'a> {
    /// Creates a runtime catalog projection.
    #[must_use]
    pub const fn new(
        harnesses: &'a [HarnessDefinition<'a>],
        models: &'a [ModelDefinition<'a>],
        runnable_harness_ids: &'a [&'a str],
    ) -> Self {
        Self {
            manifest: ModelManifest::new(harnesses, models),
            runnable_harness_ids,
        }
    }
}

/// The session-policy fields read by composer readiness and window policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadSessionPolicy<'a> {
    /// Harness/engine selected for the next launch.
    pub engine_id: &'a str,
    /// Catalog/native model selected for the next launch.
    pub model: Option<&'a str>,
    /// Opaque route-local model id, when a route-aware selection is active.
    pub model_id: Option<&'a str>,
    /// Execution/billing route, when a route-aware selection is active.
    pub provider_route_id: Option<&'a str>,
    /// Harness-native model variant, when one is active.
    pub variant_id: Option<&'a str>,
    /// Native context-window suffix requested for the next launch.
    pub context_window: Option<&'a str>,
}

impl<'a> ThreadSessionPolicy<'a> {
    /// Creates a policy with no route, variant, or context suffix.
    #[must_use]
    pub const fn new(engine_id: &'a str, model: Option<&'a str>) -> Self {
        Self {
            engine_id,
            model,
            model_id: None,
            provider_route_id: None,
            variant_id: None,
            context_window: None,
        }
    }

    /// Adds the route-aware native selection fields.
    #[must_use]
    pub const fn with_native_selection(
        mut self,
        provider_route_id: Option<&'a str>,
        model_id: Option<&'a str>,
        variant_id: Option<&'a str>,
    ) -> Self {
        self.provider_route_id = provider_route_id;
        self.model_id = model_id;
        self.variant_id = variant_id;
        self
    }

    /// Adds the requested native context-window suffix.
    #[must_use]
    pub const fn with_context_window(mut self, context_window: Option<&'a str>) -> Self {
        self.context_window = context_window;
        self
    }
}

/// Immutable origin of the run that reported a usage aggregate.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct UsageOrigin<'a> {
    /// Harness/engine that produced the usage reading, when recorded.
    pub engine_id: Option<&'a str>,
    /// Native model that produced the usage reading, when recorded.
    pub model_id: Option<&'a str>,
}

impl<'a> UsageOrigin<'a> {
    /// Creates a usage origin projection.
    #[must_use]
    pub const fn new(engine_id: Option<&'a str>, model_id: Option<&'a str>) -> Self {
        Self {
            engine_id,
            model_id,
        }
    }
}

/// The usage aggregate fields consumed by the context-window policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SurfaceUsageAggregate<'a> {
    /// Immutable provenance for the aggregate's latest context reading.
    pub context_origin: Option<UsageOrigin<'a>>,
    /// Context-window size reported by that run on the wire, when available.
    pub context_window_tokens: Option<u64>,
}

impl<'a> SurfaceUsageAggregate<'a> {
    /// Creates a usage aggregate projection.
    #[must_use]
    pub const fn new(
        context_origin: Option<UsageOrigin<'a>>,
        context_window_tokens: Option<u64>,
    ) -> Self {
        Self {
            context_origin,
            context_window_tokens,
        }
    }
}

/// Returns the exact reason a composer send must stay disabled.
///
/// Forge being offline has precedence over every catalog or policy detail.
/// Once Forge is available, an absent policy engine is allowed through, and a
/// listed runnable harness is allowed through.  An otherwise known harness is
/// named with its catalog label; an unknown harness falls back to its id.
#[must_use]
pub fn composer_send_blocked_reason(
    forge_available: bool,
    catalog: &RuntimeCatalog<'_>,
    policy: Option<&ThreadSessionPolicy<'_>>,
) -> Option<String> {
    if !forge_available {
        return Some("Forge is offline — reconnect to send".to_owned());
    }

    let policy = policy?;
    let engine_id = policy.engine_id;
    if catalog.runnable_harness_ids.contains(&engine_id) {
        return None;
    }

    let label = catalog
        .manifest
        .harnesses
        .iter()
        .find(|harness| harness.id == engine_id)
        .map_or(engine_id, |harness| harness.label);
    Some(format!(
        "{label} models are preview-only — this engine cannot run in Artisan yet"
    ))
}

/// Reports whether the usage aggregate describes the policy's current launch.
///
/// Engine identity must match.  A missing origin model is intentionally a
/// match once the engine matches, because older runs did not record models;
/// a recorded model must equal the policy model exactly.
#[must_use]
pub fn composer_context_usage_is_current(
    policy: Option<&ThreadSessionPolicy<'_>>,
    context_usage: Option<&SurfaceUsageAggregate<'_>>,
) -> bool {
    let Some(policy) = policy else { return false };
    let Some(context_usage) = context_usage else {
        return false;
    };
    let Some(origin) = context_usage.context_origin else {
        return false;
    };

    origin.engine_id == Some(policy.engine_id)
        && (origin.model_id.is_none() || origin.model_id == policy.model)
}

/// Resolves the context-window denominator for the current composer policy.
///
/// A current wire-reported value wins.  Otherwise the first exact catalog
/// model match supplies either its configurable option or its legacy scalar
/// capability.  Route-aware models match all native-selection fields, while
/// an unqualified model matches only an unqualified policy.  An absent
/// suffix selects the capability's declared default; an unknown suffix also
/// falls back to that default.  A malformed capability whose declared default
/// is not present has no option result, matching the TypeScript `.find`.
#[must_use]
pub fn composer_context_window_tokens(
    catalog: &RuntimeCatalog<'_>,
    policy: Option<&ThreadSessionPolicy<'_>>,
    context_usage: Option<&SurfaceUsageAggregate<'_>>,
) -> Option<u64> {
    if let Some(context_usage) = context_usage
        && composer_context_usage_is_current(policy, Some(context_usage))
        && let Some(tokens) = context_usage.context_window_tokens
    {
        return Some(tokens);
    }

    let policy = policy?;
    let policy_model = policy.model?;
    let model = catalog.manifest.models.iter().find(|model| {
        if model.harness != policy.engine_id || model.native_model_id != policy_model {
            return false;
        }

        match model.native_selection {
            None => policy.provider_route_id.is_none() && policy.variant_id.is_none(),
            Some(selection) => {
                Some(selection.provider_route_id) == policy.provider_route_id
                    && Some(selection.model_id) == policy.model_id.or(policy.model)
                    && selection.variant_id == policy.variant_id
            }
        }
    })?;

    let capability = model.capabilities.context_window;
    let Some(capability) = capability else {
        return model.capabilities.context_window_tokens;
    };

    let default_option = capability
        .options
        .iter()
        .find(|candidate| candidate.id == capability.default);
    let option = match policy.context_window {
        None => default_option,
        Some(suffix) => capability
            .options
            .iter()
            .find(|candidate| candidate.native_suffix == suffix)
            .or(default_option),
    };
    option.map(|option| option.tokens)
}
