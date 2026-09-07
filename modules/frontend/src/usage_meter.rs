//! Deterministic quantization for the compact provider-usage meter.
//!
//! This is the native port of
//! `modules/frontend/src/lib/identity/usage-meter.ts`. The sidebar passes the
//! returned fraction to `--meter-lit` and passes the tick count separately to
//! `--meter-ticks`; the provider tooltip remains responsible for the exact
//! remaining percentage. Keeping this policy pure means every native caller
//! paints the same 14-step meter without depending on UI state or rendering.
//!
//! A finite input is first clamped to `0..=100`, then quantized with the
//! TypeScript policy's `Math.ceil` equivalent. Zero stays unlit, while every
//! positive finite reading paints at least one tick. Non-finite input is an
//! invalid provider reading at the native boundary and deterministically falls
//! back to zero, so neither `NaN` nor infinity can reach a style value.

/// Number of discrete ticks painted by the compact provider-usage meter.
pub const USAGE_METER_SEGMENTS: u8 = 14;

/// TypeScript export spelling retained for callers that mirror the web name.
#[allow(non_upper_case_globals)]
pub const usage_meter_segments: u8 = USAGE_METER_SEGMENTS;

/// Quantizes used quota to a meter fraction in `0.0..=1.0`.
///
/// This mirrors the sidebar's
/// `Math.ceil((Math.min(100, Math.max(0, percent_used)) / 100) * 14) / 14`
/// policy. The division by `100` intentionally happens before multiplication
/// by `14`, matching JavaScript's operation order. Reassociating that
/// expression changes some IEEE-754 boundary readings, so the source order is
/// part of this native contract.
///
/// `NaN`, positive infinity, and negative infinity are treated as malformed
/// readings and return `0.0`. This makes the API total over `f64` while
/// retaining the visible-zero behavior for an absent or unusable reading.
#[must_use]
pub fn usage_segment_fraction(percent_used: f64) -> f64 {
    if !percent_used.is_finite() {
        return 0.0;
    }

    let clamped_percent = percent_used.clamp(0.0, 100.0);
    let segments = f64::from(USAGE_METER_SEGMENTS);
    (clamped_percent / 100.0 * segments).ceil() / segments
}
