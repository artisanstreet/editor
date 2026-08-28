//! Resolves the display name for the model that reported a context reading.
//!
//! This is the pure Rust counterpart of
//! `modules/frontend/src/lib/context-usage/model-name.ts`. The Svelte call
//! site passes the usage aggregate and the runtime catalog, while the current
//! thread policy is intentionally absent: a policy describes a future launch
//! and must not relabel telemetry produced by an earlier run.
//!
//! The Rust protocol crate at this port boundary does not yet expose owned
//! runtime-catalog and surface-usage values. The borrowed views below model
//! only the fields this leaf reads, leaving the eventual protocol adapter to
//! own the wider schemas. The resolver is finite over a caller-provided slice,
//! performs exact case-sensitive comparisons, and returns the catalog's own
//! name borrow without allocating or normalizing it.

#![allow(clippy::module_name_repetitions)]

/// The run provenance attached to a context-window reading.
///
/// Both identifiers must be present before a reading can identify a model.
/// `run_id` is deliberately not represented because this leaf only resolves
/// the model pair; run identity remains owned by the surrounding usage
/// projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContextOriginView<'a> {
    /// The engine or harness that produced the telemetry.
    pub engine_id: Option<&'a str>,
    /// The provider-native model identifier that produced the telemetry.
    pub model_id: Option<&'a str>,
}

impl<'a> ContextOriginView<'a> {
    /// Creates an origin view from its optional telemetry identifiers.
    #[must_use]
    pub const fn new(engine_id: Option<&'a str>, model_id: Option<&'a str>) -> Self {
        Self {
            engine_id,
            model_id,
        }
    }
}

/// The usage aggregate projection needed by model-name presentation.
///
/// Other aggregate token and scope fields are intentionally outside this
/// view: they do not affect the reporting model's display name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SurfaceUsageAggregateView<'a> {
    /// Immutable provenance of the run whose context gauge was projected.
    pub context_origin: Option<ContextOriginView<'a>>,
}

impl<'a> SurfaceUsageAggregateView<'a> {
    /// Creates a usage view with optional reporting-run provenance.
    #[must_use]
    pub const fn new(context_origin: Option<ContextOriginView<'a>>) -> Self {
        Self { context_origin }
    }
}

/// The manifest model fields used to resolve a reporting model name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelDefinitionView<'a> {
    /// Catalog harness identifier, matched to the telemetry engine exactly.
    pub harness: &'a str,
    /// Provider-native model identifier, matched to telemetry exactly.
    pub native_model_id: &'a str,
    /// Stable user-facing name returned to the caller.
    pub name: &'a str,
}

impl<'a> ModelDefinitionView<'a> {
    /// Creates a borrowed manifest model entry.
    #[must_use]
    pub const fn new(harness: &'a str, native_model_id: &'a str, name: &'a str) -> Self {
        Self {
            harness,
            native_model_id,
            name,
        }
    }
}

/// The ordered model portion of a runtime catalog manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModelManifestView<'models, 'fields> {
    /// Models in the manifest's authoritative presentation order.
    pub models: &'models [ModelDefinitionView<'fields>],
}

impl<'models, 'fields> ModelManifestView<'models, 'fields> {
    /// Creates a manifest view without sorting or deduplicating its models.
    #[must_use]
    pub const fn new(models: &'models [ModelDefinitionView<'fields>]) -> Self {
        Self { models }
    }
}

/// The runtime-catalog projection needed by this presentation leaf.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RuntimeCatalogView<'models, 'fields> {
    /// Static manifest containing the catalog's ordered model definitions.
    pub manifest: ModelManifestView<'models, 'fields>,
}

impl<'models, 'fields> RuntimeCatalogView<'models, 'fields> {
    /// Creates a runtime-catalog view from its ordered model entries.
    #[must_use]
    pub const fn new(models: &'models [ModelDefinitionView<'fields>]) -> Self {
        Self {
            manifest: ModelManifestView::new(models),
        }
    }
}

/// Returns the name of the model that reported the usage aggregate.
///
/// This mirrors the TypeScript `models.find` call exactly:
///
/// - no origin, engine, or native model id means `None`;
/// - both engine/harness and native model id must match exactly;
/// - the first matching manifest entry wins, so input order is preserved; and
/// - the returned `&str` borrows the matched catalog entry's original name.
///
/// The current thread policy is not an input by design. It may select a
/// subsequent run, but it cannot rename telemetry from the run represented by
/// `usage`.
#[must_use]
pub fn context_usage_model_name<'models, 'fields>(
    usage: SurfaceUsageAggregateView<'_>,
    runtime_catalog: RuntimeCatalogView<'models, 'fields>,
) -> Option<&'fields str> {
    let origin = usage.context_origin?;
    let engine_id = origin.engine_id?;
    let model_id = origin.model_id?;

    runtime_catalog
        .manifest
        .models
        .iter()
        .find(|model| model.harness == engine_id && model.native_model_id == model_id)
        .map(|model| model.name)
}
