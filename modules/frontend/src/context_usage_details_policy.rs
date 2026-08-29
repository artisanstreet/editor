//! Dependency-free context-window details presentation policy.
//!
//! This is the pure Rust counterpart of
//! routes/components/context-usage-details.svelte. The component has three
//! independent display facts: a rounded percentage in its prose and
//! accessible label, a compact model capacity in a second prose line, and a
//! progress fill clamped to the progress bar's range. They remain separate
//! typed values here so a renderer cannot accidentally use the fill for the
//! percentage prose or use the capacity as a percentage.
//!
//! The input boundary intentionally contains only the model name, the already
//! calculated percentage, and the context-window capacity. It does not accept
//! a prompt, token categories, or a usage breakdown: the legacy details card
//! reports totals only and Artisan does not retain the assembled prompt.
//!
//! NaN, positive infinity, and negative infinity are intentionally rejected
//! with [`ContextUsageDetailsError`]. The legacy JavaScript component receives
//! finite numbers from its caller; fabricating prose or a progress value for a
//! non-finite reading would turn an invalid reading into a false fact. Finite
//! negative, over-capacity, and fractional values remain deterministic.

use std::fmt;

/// The model name used when the caller has no model name.
pub const FALLBACK_MODEL_NAME: &str = "this model";

/// The inclusive upper bound of the determinate progress fill.
pub const PROGRESS_MAX: f64 = 100.0;

/// Why a context-details projection was rejected.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContextUsageDetailsError {
    /// The percentage was NaN, positive infinity, or negative infinity.
    NonFinitePercent,
    /// The context-window capacity was NaN, positive infinity, or negative
    /// infinity.
    NonFiniteWindowTokens,
}

impl fmt::Display for ContextUsageDetailsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonFinitePercent => formatter.write_str("context usage percent must be finite"),
            Self::NonFiniteWindowTokens => {
                formatter.write_str("context window token capacity must be finite")
            }
        }
    }
}

impl std::error::Error for ContextUsageDetailsError {}

/// The rounded percentage fact used by the details prose and accessible label.
///
/// The value is the raw percentage rounded with JavaScript Math.round
/// semantics. It is deliberately not clamped: for example, an over-100 raw
/// reading remains 120% in prose while the separate progress fill is 100.
#[derive(Clone, Debug, PartialEq)]
pub struct PercentProse {
    model_name: String,
    rounded_percent: f64,
    sentence: String,
    accessible_label: String,
}

impl PercentProse {
    /// Returns the model name used in this sentence.
    #[must_use]
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Returns the raw percentage after JavaScript-compatible rounding.
    #[must_use]
    pub const fn rounded_percent(&self) -> f64 {
        self.rounded_percent
    }

    /// Returns the complete percentage sentence from the legacy card.
    #[must_use]
    pub fn sentence(&self) -> &str {
        &self.sentence
    }

    /// Alias for `Self::sentence` for renderers that call presentation text
    /// rather than prose a sentence.
    #[must_use]
    pub fn text(&self) -> &str {
        self.sentence()
    }

    /// Returns the accessible label used by the progress trigger.
    #[must_use]
    pub fn accessible_label(&self) -> &str {
        &self.accessible_label
    }
}

/// The model-capacity prose fact shown below `PercentProse`.
///
/// Its compact token text is intentionally a separate fact from the rounded
/// percentage. Capacity is formatted as a whole compact English unit and does
/// not expose any prompt or category information.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ModelCapacityProse {
    model_name: String,
    compact_window_tokens: String,
    sentence: String,
}

impl ModelCapacityProse {
    /// Returns the model name used in this sentence.
    #[must_use]
    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Returns the compact, whole-unit window capacity without the word
    /// tokens.
    #[must_use]
    pub fn compact_window_tokens(&self) -> &str {
        &self.compact_window_tokens
    }

    /// Alias for `Self::compact_window_tokens` for capacity-oriented callers.
    #[must_use]
    pub fn compact_tokens(&self) -> &str {
        self.compact_window_tokens()
    }

    /// Returns the complete model-capacity sentence from the legacy card.
    #[must_use]
    pub fn sentence(&self) -> &str {
        &self.sentence
    }

    /// Alias for `Self::sentence` for renderers that call presentation text
    /// rather than prose a sentence.
    #[must_use]
    pub fn text(&self) -> &str {
        self.sentence()
    }
}

/// The determinate progress-bar fill fact.
///
/// The raw percentage is clamped to 0..=100, exactly as the Svelte
/// Math.min(100, Math.max(0, percent)) expression does. It remains distinct
/// from `PercentProse`, whose rounded value is intentionally not clamped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProgressFill {
    value: f64,
    max: f64,
}

impl ProgressFill {
    /// Returns the clamped value passed to the progress indicator.
    #[must_use]
    pub const fn value(self) -> f64 {
        self.value
    }

    /// Returns the progress indicator's fixed maximum (100).
    #[must_use]
    pub const fn max(self) -> f64 {
        self.max
    }
}

/// Complete, totals-only projection of the context usage details card.
///
/// The two prose fields are intentionally different types. A later renderer
/// may place them on separate lines just as the Svelte card does, while the
/// fill remains a numeric progress fact rather than a prose value.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextUsageDetails {
    /// Rounded raw-percent prose and accessible-label facts.
    pub percent_prose: PercentProse,
    /// Whole-unit model-capacity prose fact.
    pub model_capacity_prose: ModelCapacityProse,
    /// Clamped determinate progress fill.
    pub progress_fill: ProgressFill,
}

impl ContextUsageDetails {
    /// Projects the finite values used by the legacy details component.
    ///
    /// None means the JavaScript `model_name` was absent and selects
    /// `FALLBACK_MODEL_NAME`. Some("") remains an explicitly supplied empty
    /// name. Negative percentages remain negative in the prose and clamp to
    /// zero only in the progress fill; percentages above 100 remain above 100
    /// in the prose and clamp to 100 only in the fill.
    ///
    /// # Errors
    ///
    /// Returns `NonFinitePercent` or `NonFiniteWindowTokens` for a non-finite
    /// input. No presentation fact is constructed for such a reading.
    pub fn new(
        model_name: Option<&str>,
        percent: f64,
        window_tokens: f64,
    ) -> Result<Self, ContextUsageDetailsError> {
        if !percent.is_finite() {
            return Err(ContextUsageDetailsError::NonFinitePercent);
        }
        if !window_tokens.is_finite() {
            return Err(ContextUsageDetailsError::NonFiniteWindowTokens);
        }

        let model_name = model_name.unwrap_or(FALLBACK_MODEL_NAME).to_owned();
        let rounded_percent = round_like_javascript_math_round(percent);
        let rounded_text = format_rounded_integer(rounded_percent);
        let compact_window_tokens = format_compact_tokens_finite(window_tokens);

        Ok(Self {
            percent_prose: PercentProse {
                sentence: format!("The context window for {model_name} is {rounded_text}% full."),
                accessible_label: format!("Context window {rounded_text}% full"),
                model_name: model_name.clone(),
                rounded_percent,
            },
            model_capacity_prose: ModelCapacityProse {
                sentence: format!(
                    "{model_name} has a context window of {compact_window_tokens} tokens."
                ),
                model_name,
                compact_window_tokens,
            },
            progress_fill: ProgressFill {
                value: percent.clamp(0.0, PROGRESS_MAX),
                max: PROGRESS_MAX,
            },
        })
    }

    /// Alias for `Self::new` that makes the rejection boundary explicit at
    /// call sites that already use try_* naming.
    ///
    /// # Errors
    ///
    /// Returns the same error as `Self::new` when `percent` or
    /// `window_tokens` is non-finite.
    pub fn try_new(
        model_name: Option<&str>,
        percent: f64,
        window_tokens: f64,
    ) -> Result<Self, ContextUsageDetailsError> {
        Self::new(model_name, percent, window_tokens)
    }

    /// Returns the model name shared by the two prose facts.
    #[must_use]
    pub fn model_name(&self) -> &str {
        self.percent_prose.model_name()
    }

    /// Returns the percentage prose fact.
    #[must_use]
    pub const fn percent(&self) -> &PercentProse {
        &self.percent_prose
    }

    /// Returns the model-capacity prose fact.
    #[must_use]
    pub const fn capacity(&self) -> &ModelCapacityProse {
        &self.model_capacity_prose
    }

    /// Returns the numeric progress-fill fact.
    #[must_use]
    pub const fn fill(&self) -> ProgressFill {
        self.progress_fill
    }
}

/// Projects one context-details card without constructing a policy object.
///
/// # Errors
///
/// Returns the same error as `ContextUsageDetails::new` when `percent`
/// or `window_tokens` is non-finite.
pub fn context_usage_details(
    model_name: Option<&str>,
    percent: f64,
    window_tokens: f64,
) -> Result<ContextUsageDetails, ContextUsageDetailsError> {
    ContextUsageDetails::new(model_name, percent, window_tokens)
}

/// Alias naming the module's policy boundary explicitly.
///
/// # Errors
///
/// Returns the same error as `context_usage_details` when `percent` or
/// `window_tokens` is non-finite.
pub fn project_context_usage_details(
    model_name: Option<&str>,
    percent: f64,
    window_tokens: f64,
) -> Result<ContextUsageDetails, ContextUsageDetailsError> {
    context_usage_details(model_name, percent, window_tokens)
}

/// Formats a finite context-window capacity with the legacy English compact
/// formatter's whole-unit behavior.
///
/// The formatter uses decimal units (K, M, B, and T), rounds with
/// Intl.NumberFormat's positive half-expand rule, and promotes a rounded
/// 1000K to 1M (likewise 1000M to 1B). Values below 1000 are rounded to whole
/// tokens. Negative values preserve their sign, including -0 when a negative
/// fractional input rounds to zero. No locale or external formatter is
/// consulted.
///
/// # Errors
///
/// Returns `NonFiniteWindowTokens` for NaN or either infinity. This is the same
/// intentional rejection used by the full projection.
pub fn format_compact_tokens(value: f64) -> Result<String, ContextUsageDetailsError> {
    if !value.is_finite() {
        return Err(ContextUsageDetailsError::NonFiniteWindowTokens);
    }
    Ok(format_compact_tokens_finite(value))
}

/// Alias for callers that name the value by its role in the card.
///
/// # Errors
///
/// Returns the same error as `format_compact_tokens` for a non-finite
/// capacity.
pub fn format_compact_window_tokens(value: f64) -> Result<String, ContextUsageDetailsError> {
    format_compact_tokens(value)
}

const COMPACT_DIVISORS: [f64; 5] = [
    1.0,
    1_000.0,
    1_000_000.0,
    1_000_000_000.0,
    1_000_000_000_000.0,
];
const COMPACT_SUFFIXES: [&str; 5] = ["", "K", "M", "B", "T"];

fn format_compact_tokens_finite(value: f64) -> String {
    let negative = value.is_sign_negative();
    let magnitude = value.abs();
    let mut unit_index = initial_compact_unit(magnitude);

    loop {
        let scaled = magnitude / COMPACT_DIVISORS[unit_index];
        let rounded = round_half_expand_positive(scaled);

        // Intl compact notation selects the next unit when rounding would
        // create a fourth digit: 999.5 -> 1K and 999.5K -> 1M.
        if rounded >= 1_000.0 && unit_index + 1 < COMPACT_DIVISORS.len() {
            unit_index += 1;
            continue;
        }

        let digits = format!("{rounded:.0}");
        let integer = if negative {
            format!("-{digits}")
        } else {
            digits
        };
        let integer = if integer == "-0" || integer == "0" {
            if negative {
                "-0".to_owned()
            } else {
                "0".to_owned()
            }
        } else {
            group_compact_integer(integer, unit_index)
        };
        return format!("{integer}{}", COMPACT_SUFFIXES[unit_index]);
    }
}

fn initial_compact_unit(magnitude: f64) -> usize {
    if magnitude < COMPACT_DIVISORS[1] {
        0
    } else if magnitude < COMPACT_DIVISORS[2] {
        1
    } else if magnitude < COMPACT_DIVISORS[3] {
        2
    } else if magnitude < COMPACT_DIVISORS[4] {
        3
    } else {
        4
    }
}

fn group_compact_integer(integer: String, unit_index: usize) -> String {
    // Compact notation's default English grouping is min2: a four-digit
    // quantity such as 1000T stays ungrouped, while 10000T is 10,000T.
    if unit_index != 4 || integer.len() <= 4 {
        return integer;
    }

    let (sign, digits) = integer
        .strip_prefix('-')
        .map_or(("", integer.as_str()), |digits| ("-", digits));
    let first_group_len = digits.len() % 3;
    let first_group_len = if first_group_len == 0 {
        3
    } else {
        first_group_len
    };
    let mut grouped = String::with_capacity(integer.len() + (digits.len() - 1) / 3);
    grouped.push_str(sign);
    for (index, digit) in digits.chars().enumerate() {
        if index < first_group_len {
            grouped.push(digit);
            continue;
        }
        if (index - first_group_len).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// Positive half-expand rounding used by Intl.NumberFormat for capacities.
fn round_half_expand_positive(value: f64) -> f64 {
    let lower = value.floor();
    if value - lower >= 0.5 {
        lower + 1.0
    } else {
        lower
    }
}

/// JavaScript Math.round: halfway values go toward positive infinity.
fn round_like_javascript_math_round(value: f64) -> f64 {
    let lower = value.floor();
    if value - lower >= 0.5 {
        lower + 1.0
    } else {
        lower
    }
}

fn format_rounded_integer(value: f64) -> String {
    // JavaScript string interpolation renders both +0 and -0 as 0.
    if value == 0.0 {
        "0".to_owned()
    } else {
        format!("{value:.0}")
    }
}
