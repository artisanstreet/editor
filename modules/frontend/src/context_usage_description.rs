//! Deterministic context-window usage descriptions.
//!
//! This is the pure Rust port of
//! `modules/frontend/src/lib/context-usage/description.ts`. The TypeScript
//! gauge receives a complete `SurfaceUsageAggregate`, but its spoken
//! description reads only the four token fields represented by
//! [`ContextUsageAggregate`]. The gauge resolves its required window size
//! separately, so `window_tokens` is an explicit argument here as it is at the
//! real call site.
//!
//! The protocol schema constrains these token values to nonnegative integers.
//! This projection uses `u64` rather than floating point: every value in the
//! supported Rust domain remains exact, including large values, and formatting
//! is deterministic regardless of process locale. Values from JavaScript's
//! exact integer range are a subset of this domain.

/// The token portion of one protocol `SurfaceUsageAggregate` needed for its
/// accessible context-window description.
///
/// The protocol's `scope`, `scope_id`, context provenance, cost, and other
/// fields do not affect this sentence and intentionally remain outside this
/// presentation projection. Every `Some` value is a nonnegative whole token
/// count; `None` means that the provider omitted that metric.
// Keep the protocol's exact field names at this projection boundary.
#[allow(clippy::struct_field_names)]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ContextUsageAggregate {
    /// Latest point-in-time context-window usage, if reported.
    pub context_tokens: Option<u64>,
    /// Input tokens, if reported.
    pub input_tokens: Option<u64>,
    /// Cached input tokens, if reported.
    pub cached_input_tokens: Option<u64>,
    /// Output tokens, if reported.
    pub output_tokens: Option<u64>,
}

/// Formats a nonnegative token count with English thousands grouping.
///
/// This is the integer equivalent of `Intl.NumberFormat("en")` for the
/// supported token domain: decimal digits are grouped from the right in sets
/// of three with `,` separators. It does not round, compact, localize, or
/// convert through `f64`, so every `u64` value is represented exactly.
#[must_use]
pub fn format_token_count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + (digits.len() - 1) / 3);

    for (index, digit) in digits.chars().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }

    grouped
}

/// States the complete context-window reading in the exact sentence order
/// used by the TypeScript gauge's screen-reader description.
///
/// The context total defaults to zero when `context_tokens` is `None`. The
/// optional breakdown is emitted in the fixed order `Input`, `Cached input`,
/// `Output`; omitted fields are skipped, while reported zeroes remain visible.
/// When all three breakdown fields are omitted, the exact suffix is
/// `No detailed token breakdown is available.`
#[must_use]
pub fn context_usage_description(usage: &ContextUsageAggregate, window_tokens: u64) -> String {
    let mut description = format!(
        "Context window contains {} of {} tokens.",
        format_token_count(usage.context_tokens.unwrap_or(0)),
        format_token_count(window_tokens),
    );
    let mut has_breakdown = false;

    for (label, value) in [
        ("Input", usage.input_tokens),
        ("Cached input", usage.cached_input_tokens),
        ("Output", usage.output_tokens),
    ] {
        let Some(value) = value else {
            continue;
        };

        has_breakdown = true;
        description.push(' ');
        description.push_str(label);
        description.push_str(": ");
        description.push_str(&format_token_count(value));
        description.push_str(" tokens.");
    }

    if !has_breakdown {
        description.push_str(" No detailed token breakdown is available.");
    }

    description
}
