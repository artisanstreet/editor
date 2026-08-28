//! Pure speed-tier presentation and selection policy for the frontend.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/engine/speed-presentation.ts`. The TypeScript
//! leaf only reads an option's `id` and `label`, but its callers also define
//! the surrounding rules for unavailable options, defaults, native-value
//! lookup, and the order in which options are displayed. Those rules are kept
//! here as data-only helpers so a later renderer can use the same policy
//! without importing catalog, engine, or provider behavior.
//!
//! The policy is deliberately independent of reasoning effort. A speed id is
//! not a thinking-level id, and no helper here probes availability, starts an
//! engine, or changes a service tier.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// Presentation applied to one speed option in a model-picker trigger or
/// dispatch badge.
///
/// This mirrors `SpeedOptionPresentation` in the TypeScript leaf. Branded
/// accelerated tiers replace the catalog label; every other tier retains its
/// own label verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeedOptionPresentation {
    /// Tailwind classes for the speed label. Empty for unbranded tiers.
    pub class_name: &'static str,
    /// Human-readable label shown beside the model name.
    pub label: String,
}

/// The catalog fields consumed by the speed presentation policy.
///
/// `SpeedOption` in the catalog carries additional pricing and provenance
/// metadata. The native leaf retains only the fields its frontend call sites
/// read: identity, label/description, wire value, default state, and dynamic
/// availability. A disabled reason is represented by `Some`; its text is
/// intentionally not interpreted by this pure policy.
#[must_use = "use the option in a speed policy or presentation"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpeedOption {
    /// Stable catalog identifier used by picker selection.
    pub id: String,
    /// Catalog label used for ordinary and unknown tiers.
    pub label: String,
    /// Native value written to `service_tier` and used by dispatch lookup.
    pub native_value: String,
    /// Exact tooltip description retained for the eventual renderer.
    pub description: String,
    /// Whether this option is the model's curated default.
    pub default: bool,
    /// Dynamic unavailability and its provider/catalog reason, if any.
    pub disabled: Option<String>,
}

impl SpeedOption {
    /// Creates a speed option from the fields this policy observes.
    #[must_use = "use the constructed speed option"]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        native_value: impl Into<String>,
        description: impl Into<String>,
        default: bool,
        disabled: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            native_value: native_value.into(),
            description: description.into(),
            default,
            disabled,
        }
    }
}

/// Tailwind classes used for the exact `fast` presentation.
pub const FAST_CLASS: &str = "text-amber-600 dark:text-amber-400";
/// Exact label used for the `fast` presentation.
pub const FAST_LABEL: &str = "Fast";
/// Tailwind classes used for the exact `superfast` presentation.
pub const SUPERFAST_CLASS: &str =
    "bg-linear-to-r from-purple-500 to-green-500 bg-clip-text text-transparent";
/// Exact label used for the `superfast` presentation.
pub const SUPERFAST_LABEL: &str = "Superfast";
/// Fallback picker id used when no selectable speed resolves.
pub const DEFAULT_SPEED_ID: &str = "standard";
/// Fallback wire value used when no selectable speed resolves.
pub const DEFAULT_NATIVE_VALUE: &str = "standard";

/// Returns the exact trigger/badge presentation for one speed option.
///
/// The comparisons intentionally remain case-sensitive and perform no
/// trimming, matching TypeScript's strict equality checks. Only `fast` and
/// `superfast` receive branded output; every other id, including `standard`
/// and unknown future ids, returns an empty class and the option's own label.
#[must_use = "use the rendered speed presentation"]
pub fn speed_option_presentation(option: &SpeedOption) -> SpeedOptionPresentation {
    match option.id.as_str() {
        "fast" => SpeedOptionPresentation {
            class_name: FAST_CLASS,
            label: FAST_LABEL.to_owned(),
        },
        "superfast" => SpeedOptionPresentation {
            class_name: SUPERFAST_CLASS,
            label: SUPERFAST_LABEL.to_owned(),
        },
        _ => SpeedOptionPresentation {
            class_name: "",
            label: option.label.clone(),
        },
    }
}

/// Returns whether a catalog option may be selected.
///
/// The Svelte policy controls filter out every option with a disabled reason;
/// an empty reason is still a disabled state when represented by `Some`.
#[must_use]
pub fn is_available(option: &SpeedOption) -> bool {
    option.disabled.is_none()
}

/// Returns available options without changing their catalog/display order.
#[must_use]
pub fn available_speed_options(options: &[SpeedOption]) -> Vec<&SpeedOption> {
    options
        .iter()
        .filter(|option| is_available(option))
        .collect()
}

/// Finds the first option with an exact catalog id.
///
/// This is a raw lookup: availability is applied by selection and
/// presentation helpers at the call site that needs it.
#[must_use]
pub fn speed_by_id<'a>(options: &'a [SpeedOption], id: &str) -> Option<&'a SpeedOption> {
    options.iter().find(|option| option.id == id)
}

/// Finds the first option with an exact native wire value.
///
/// Native values are not normalized, trimmed, or case-folded. The raw lookup
/// is useful for authoritative dispatch policy; callers that choose a value
/// for the picker should use [`selected_speed_by_native_value`].
#[must_use]
pub fn speed_by_native_value<'a>(
    options: &'a [SpeedOption],
    native_value: &str,
) -> Option<&'a SpeedOption> {
    options
        .iter()
        .find(|option| option.native_value == native_value)
}

/// Resolves the model's selectable default speed.
///
/// The first available option marked `default` wins. If a catalog snapshot
/// has no available marked default, the first available option is the safe
/// deterministic fallback. Empty or entirely disabled input returns `None`;
/// callers may then use [`DEFAULT_SPEED_ID`] and [`DEFAULT_NATIVE_VALUE`].
#[must_use]
pub fn default_speed(options: &[SpeedOption]) -> Option<&SpeedOption> {
    options
        .iter()
        .find(|option| is_available(option) && option.default)
        .or_else(|| options.iter().find(|option| is_available(option)))
}

/// Resolves a selectable option from a stored picker id.
///
/// An exact available id wins. Unknown, missing, or disabled ids fall back to
/// the available model default and then to the first available option.
#[must_use]
pub fn selected_speed_by_id<'a>(
    options: &'a [SpeedOption],
    preferred_id: &str,
) -> Option<&'a SpeedOption> {
    options
        .iter()
        .find(|option| is_available(option) && option.id == preferred_id)
        .or_else(|| default_speed(options))
}

/// Resolves a selectable option from a stored native service-tier value.
///
/// An exact available native value wins. Retired, unknown, or disabled values
/// fall back to the available model default and then to the first available
/// option, matching draft seeding and the model controls.
#[must_use]
pub fn selected_speed_by_native_value<'a>(
    options: &'a [SpeedOption],
    preferred_native_value: &str,
) -> Option<&'a SpeedOption> {
    options
        .iter()
        .find(|option| is_available(option) && option.native_value == preferred_native_value)
        .or_else(|| default_speed(options))
}

/// Resolves the speed id written by `SyncAuthoritativePolicy`.
///
/// This is intentionally a different policy from
/// [`selected_speed_by_native_value`]. The authoritative sync expression
/// first finds an exact native value without checking `disabled`, then finds
/// the first default without checking `disabled`, and finally assigns the
/// literal `"standard"` when neither option exists. The literal fallback is
/// returned even when no `standard` option exists in the catalog.
#[must_use = "use the synchronized speed id"]
pub fn authoritative_speed_id(options: &[SpeedOption], service_tier: &str) -> String {
    speed_by_native_value(options, service_tier)
        .or_else(|| options.iter().find(|option| option.default))
        .map_or_else(|| DEFAULT_SPEED_ID.to_owned(), |option| option.id.clone())
}

/// Resolves the model-picker trigger's optional speed badge.
///
/// The trigger adds no word for the model's own default. The selected id is
/// normally produced by the available-selection policy, but the trigger
/// expression itself does not inspect `disabled`; a stale disabled selection
/// is still rendered when its id is selected.
#[must_use]
pub fn trigger_speed_presentation(
    options: &[SpeedOption],
    selected_id: &str,
) -> Option<SpeedOptionPresentation> {
    let selected = speed_by_id(options, selected_id)?;
    if selected.default {
        return None;
    }
    Some(speed_option_presentation(selected))
}

/// Resolves the optional speed badge for a dispatched model.
///
/// Dispatch presentation uses the policy's native value and hides the
/// model's own default. The lookup and comparisons remain exact; an unknown
/// value produces no badge. As in the source dispatch expression, `disabled`
/// does not affect this presentation lookup.
#[must_use]
pub fn dispatch_speed_presentation(
    options: &[SpeedOption],
    service_tier: &str,
) -> Option<SpeedOptionPresentation> {
    let option = speed_by_native_value(options, service_tier)?;
    if option.default {
        return None;
    }
    Some(speed_option_presentation(option))
}
