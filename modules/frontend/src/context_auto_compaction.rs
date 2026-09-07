//! Pure context auto-compaction policy.
//!
//! This is the dependency-free Rust port of
//! `modules/frontend/src/lib/context-usage/auto-compaction.ts`. The policy is
//! deliberately separate from catalog lookup, current thread policy, and
//! presentation: callers provide the window resolved for the reading and the
//! reading carries the immutable origin of the run that reported it.
//!
//! A non-finite or non-positive window is invalid and reports the conservative
//! window boundary (`100%`). A non-finite usage value or arithmetic result
//! cannot prove imminence, so it returns `false`. These guards make the policy
//! total over runtime numeric input without allowing NaN or infinity to leak
//! into a decision.

/// Codex's documented compaction threshold as a percentage of its window.
pub const CODEX_COMPACTION_PERCENT: f64 = 90.0;

/// Claude Sonnet 5's documented default compaction capacity in tokens.
pub const CLAUDE_SONNET_5_COMPACTION_TOKENS: f64 = 967_000.0;

/// The fallback threshold when no documented engine policy can be applied.
pub const UNKNOWN_COMPACTION_PERCENT: f64 = 100.0;

/// Immutable engine/model identity carried by a context usage observation.
///
/// The origin describes the run that reported the reading. It is intentionally
/// distinct from a thread's current launch policy, which may already point at
/// another engine or model by the time the aggregate is rendered.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextUsageOrigin<'a> {
    /// Provider harness that reported the observation, when known.
    pub engine_id: Option<&'a str>,
    /// Native model id that reported the observation, when known.
    pub model_id: Option<&'a str>,
}

impl<'a> ContextUsageOrigin<'a> {
    /// Creates an origin without allocating or normalizing either id.
    #[must_use]
    pub const fn new(engine_id: Option<&'a str>, model_id: Option<&'a str>) -> Self {
        Self {
            engine_id,
            model_id,
        }
    }
}

/// Minimal usage aggregate needed by the auto-compaction policy.
///
/// The protocol aggregate contains other token, cost, and scope fields; none
/// of them affects this decision. `None` means the provider did not report the
/// corresponding value. The numeric representation remains `f64` at this
/// pure boundary so malformed runtime values can be rejected explicitly rather
/// than causing a panic or an indeterminate comparison.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ContextUsageAggregate<'a> {
    /// Origin of the run that reported the context reading.
    pub context_origin: Option<ContextUsageOrigin<'a>>,
    /// Latest reported context usage, in tokens.
    pub context_tokens: Option<f64>,
}

impl<'a> ContextUsageAggregate<'a> {
    /// Creates a minimal aggregate without copying or validating its values.
    #[must_use]
    pub const fn new(
        context_origin: Option<ContextUsageOrigin<'a>>,
        context_tokens: Option<f64>,
    ) -> Self {
        Self {
            context_origin,
            context_tokens,
        }
    }
}

/// Model and denominator inputs used to resolve an engine's threshold.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoCompactionModel<'a> {
    /// Exact harness identifier, such as `"codex"` or `"claude"`.
    pub harness: &'a str,
    /// Exact native model identifier, when the harness reported one.
    pub native_model_id: Option<&'a str>,
    /// Window used as the denominator for the context gauge.
    pub window_tokens: f64,
}

impl<'a> AutoCompactionModel<'a> {
    /// Creates model inputs without normalizing the harness or model id.
    #[must_use]
    pub const fn new(
        harness: &'a str,
        native_model_id: Option<&'a str>,
        window_tokens: f64,
    ) -> Self {
        Self {
            harness,
            native_model_id,
            window_tokens,
        }
    }
}

/// Returns the percentage at which the supplied engine begins compacting.
///
/// The percentage is measured against `window_tokens`, the same denominator
/// used by the context gauge. Unknown or incomplete harnesses conservatively
/// return `100%`. Claude is normally `100%`; the exact native id
/// `claude-sonnet-5` uses `min(967_000, window_tokens)` before converting its
/// capacity to a percentage. The harness and model comparisons are exact and
/// case-sensitive, matching the TypeScript policy.
#[must_use]
pub fn context_auto_compaction_percent(model: AutoCompactionModel<'_>) -> f64 {
    if !model.window_tokens.is_finite() || model.window_tokens <= 0.0 {
        return UNKNOWN_COMPACTION_PERCENT;
    }

    match model.harness {
        "codex" => CODEX_COMPACTION_PERCENT,
        "claude" => {
            let compaction_tokens = if model.native_model_id == Some("claude-sonnet-5") {
                CLAUDE_SONNET_5_COMPACTION_TOKENS.min(model.window_tokens)
            } else {
                model.window_tokens
            };
            (compaction_tokens / model.window_tokens * 100.0).min(100.0)
        }
        _ => UNKNOWN_COMPACTION_PERCENT,
    }
}

/// Resolves the threshold from the run that reported `usage`.
///
/// The aggregate's `context_origin` is the only engine/model identity read by
/// this function. A caller's current thread policy is intentionally absent: it
/// describes a future launch and must not reinterpret existing telemetry.
#[must_use]
pub fn context_usage_auto_compaction_percent(
    usage: Option<&ContextUsageAggregate<'_>>,
    window_tokens: f64,
) -> f64 {
    let origin = usage.and_then(|aggregate| aggregate.context_origin);
    context_auto_compaction_percent(AutoCompactionModel::new(
        origin.and_then(|value| value.engine_id).unwrap_or(""),
        origin.and_then(|value| value.model_id),
        window_tokens,
    ))
}

/// Returns whether the reported context is at or beyond its compaction point.
///
/// Missing usage/window values, non-positive windows, non-finite inputs, and a
/// non-finite ratio are all treated as not imminent. Otherwise the comparison
/// is inclusive: a reading exactly at the resolved threshold returns `true`.
#[must_use]
pub fn context_compaction_is_imminent(
    usage: Option<&ContextUsageAggregate<'_>>,
    window_tokens: Option<f64>,
) -> bool {
    let Some(aggregate) = usage else {
        return false;
    };
    let Some(used) = aggregate.context_tokens else {
        return false;
    };
    let Some(window) = window_tokens else {
        return false;
    };
    if !used.is_finite() || !window.is_finite() || window <= 0.0 {
        return false;
    }

    let percent = used / window * 100.0;
    percent.is_finite() && percent >= context_usage_auto_compaction_percent(Some(aggregate), window)
}
