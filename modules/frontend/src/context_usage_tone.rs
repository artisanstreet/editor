//! Bounded context-usage gauge tone policy.
//!
//! Pure port of the reached `gauge-tone.ts` contract
//! (`modules/frontend/src/lib/context-usage/gauge-tone.ts`) and its call-site
//! normalisation. The TypeScript source treats the gauge as a continuous
//! two-leg ramp rather than three discrete states, keeps the first two anchors
//! fixed per reading, and anchors only the red leg's endpoint to the engine's
//! documented compaction threshold. This module reproduces that geometry with
//! finite-safe, deterministic Rust and without UI, engine, or transport side
//! effects.
//!
//! ## Anchors
//!
//! * `WARN_FROM = 50.0` — where the window stops being unremarkable. Caller
//!   supplied; never derived from the engine.
//! * `DANGER_FROM = 80.0` — where the ramp reddens. Also caller supplied;
//!   fixed.
//! * `compaction_percent` — engine-derived endpoint of the red leg. Unknown
//!   and non-finite values fall back to `100.0` (the window itself), matching
//!   `auto-compaction.ts`'s undocumented-engine case. Values at or below
//!   `DANGER_FROM` collapse the red leg to the inclusive step
//!   `percent >= 80 ? 100 : 0`.
//!
//! ## Ratio handling
//!
//! `context_usage_percent` mirrors `thread-composer.svelte:117` and
//! `auto-compaction.ts:120`: ` (used / window) * 100` clamped to `0..=100`.
//! Window `<= 0`, non-finite, or missing inputs are `None` (no gauge), which
//! is what `controls.svelte`'s `has_context_reading` branch and the details
//! card's `compact_tokens.format` treat as "no reading". Over-capacity
//! (`used > window`) saturates at `100` rather than reporting `>100`. Negative
//! token counts, while not produced by the domain, clamp to `0` instead of
//! panicking.
//!
//! ## Display / accessibility
//!
//! The ring and the details card derive the same two display decisions from
//! one finite percent: `Math.round(percent)` for the spoken
//! `aria-label="Context window N% full"` and `Math.min(100, Math.max(0,
//! percent))` for the bar fill. Both are exposed here as pure helpers so
//! callers cannot drift apart. Token compact formatting (`Intl.NumberFormat`
//! compact) is a locale presentation concern left to the view.
//!
//! ## Finite safety
//!
//! Every public function is total over `f64`: non-finite `percent`,
//! `compaction_percent`, or token inputs are handled deterministically and
//! never propagate `NaN` or `Infinity` into a tone, fill, or label. The ramp
//! itself is `Math.round(clamp(0..=100, (percent-from)/(to-from)*100))`; the
//! Rust form clamps before rounding just as the TypeScript does.

/// Where the gauge stops being calm blue independent of any engine.
pub const WARN_FROM: f64 = 50.0;

/// Where the gauge reddens independent of any engine.
pub const DANGER_FROM: f64 = 80.0;

/// Continuous tone mix expressed as leg completions in percent.
///
/// `warn` is the `WARN_FROM -> DANGER_FROM` leg, `danger` is the
/// `DANGER_FROM -> max(compaction_percent, DANGER_FROM)` leg. Both are
/// integers `0..=100` after the TypeScript `Math.round` step.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct GaugeToneMix {
    /// Percent of the way from the warning tone to the danger tone.
    pub danger: u8,
    /// Percent of the way from the calm tone to the warning tone.
    pub warn: u8,
}

/// How far along each leg of the blue → warning → danger ramp a reading
/// sits. Direct port of `ContextGaugeToneMix`.
///
/// `percent` is already the ratio `used / window * 100` (see
/// [`context_usage_percent`]). `compaction_percent` is the engine's
/// documented auto-compaction threshold as a percent of the same window,
/// with unknown engines reporting `100.0`. Non-finite `percent` returns calm
/// `{0,0}`; non-finite `compaction_percent` is treated as `100.0`.
///
/// The ramp is `round(clamp(0..=100, (percent-from)/(to-from)*100))`. When
/// `to <= from` (i.e. `compaction_percent <= 80`) it is the inclusive step
/// `percent >= to ? 100 : 0`, matching the TypeScript `to <= from` branch.
#[must_use]
pub fn context_gauge_tone_mix(percent: f64, compaction_percent: f64) -> GaugeToneMix {
    let warn = ramp(WARN_FROM, DANGER_FROM, percent);
    let effective_to = if compaction_percent.is_finite() {
        compaction_percent.max(DANGER_FROM)
    } else {
        100.0
    };
    let danger = ramp(DANGER_FROM, effective_to, percent);
    GaugeToneMix { danger, warn }
}

/// Alias matching the TypeScript export name exactly.
#[must_use]
pub fn gauge_tone_mix(percent: f64, compaction_percent: f64) -> GaugeToneMix {
    context_gauge_tone_mix(percent, compaction_percent)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn ramp(from: f64, to: f64, percent: f64) -> u8 {
    if !percent.is_finite() || !from.is_finite() || !to.is_finite() {
        return 0;
    }
    if to <= from {
        return if percent >= to { 100 } else { 0 };
    }
    let raw = (percent - from) / (to - from) * 100.0;
    if !raw.is_finite() {
        return 0;
    }
    raw.clamp(0.0, 100.0).round().clamp(0.0, 100.0) as u8
}

/// Bounded context-window fullness as a percent `0.0..=100.0`.
///
/// Mirrors `thread-composer.svelte:120` and `auto-compaction.ts:120`:
/// ` (context_tokens / window_tokens) * 100` clamped to `0..=100` with
/// `Math.min(100, ...)`. Returns `None` when no gauge may be shown:
/// `window_tokens` is `None`, `<= 0`, or non-finite, or either token count
/// is non-finite. Over-capacity saturates at `100.0`. Negative `used` clamps
/// to `0.0`.
#[must_use]
pub fn context_usage_percent(context_tokens: f64, window_tokens: f64) -> Option<f64> {
    if !context_tokens.is_finite() || !window_tokens.is_finite() {
        return None;
    }
    if window_tokens <= 0.0 {
        return None;
    }
    if context_tokens < 0.0 {
        return Some(0.0);
    }
    let raw = context_tokens / window_tokens * 100.0;
    if !raw.is_finite() {
        return None;
    }
    Some(raw.clamp(0.0, 100.0))
}

/// Same as [`context_usage_percent`] but over optional token gauges as they
/// arrive from the provider wire (`SurfaceUsageAggregate` may omit either
/// field). `None` in either position means unknown → no percent.
#[must_use]
pub fn context_usage_percent_opt(
    context_tokens: Option<f64>,
    window_tokens: Option<f64>,
) -> Option<f64> {
    let used = context_tokens?;
    let window = window_tokens?;
    context_usage_percent(used, window)
}

/// Clamped fill share for the determinate progress bar.
///
/// Port of `context-usage-details.svelte:34`:
/// `Math.min(100, Math.max(0, percent))`. Non-finite input becomes `0.0`.
#[must_use]
pub fn gauge_fill_percent(percent: f64) -> f64 {
    if !percent.is_finite() {
        return 0.0;
    }
    percent.clamp(0.0, 100.0)
}

/// Rounded percent for the spoken gauge label.
///
/// Port of the `aria-label="Context window N% full"` sites in
/// `context-usage-gauge.svelte:47` and `context-usage-details.svelte:47,56`
/// where `N` is `Math.round(percent)`. Clamped `0..=100` after rounding,
/// non-finite → `0`.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
#[must_use]
pub fn gauge_label_percent(percent: f64) -> u8 {
    if !percent.is_finite() {
        return 0;
    }
    percent.round().clamp(0.0, 100.0) as u8
}

/// Rounded percent for the `ContextUsageDetails` prose
/// (`"Context Window … is N% full."`). Identical rounding to
/// [`gauge_label_percent`].
#[must_use]
pub fn gauge_display_percent(percent: f64) -> u8 {
    gauge_label_percent(percent)
}

/// Discrete tone for callers that collapse the ramp to a state name.
///
/// The ramp itself stays continuous ([`GaugeToneMix`]); this helper only
/// names the leg that has been entered for badge or copy purposes. Below
/// `50` is calm, `50..80` is warning, at or above `80` is danger, inclusive
/// at the lower bound of each leg. Non-finite → calm.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum ContextTone {
    /// Below `50` — calm blue.
    Calm,
    /// `50..80` — warming.
    Warning,
    /// At or above `80` — danger red.
    Danger,
}

impl ContextTone {
    /// Derives the discrete tone from a finite percent.
    #[must_use]
    pub fn from_percent(percent: f64) -> Self {
        if !percent.is_finite() {
            return Self::Calm;
        }
        if percent >= DANGER_FROM {
            Self::Danger
        } else if percent >= WARN_FROM {
            Self::Warning
        } else {
            Self::Calm
        }
    }

    /// Human label carried for completeness; not rendered here.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Calm => "calm",
            Self::Warning => "warning",
            Self::Danger => "danger",
        }
    }
}
