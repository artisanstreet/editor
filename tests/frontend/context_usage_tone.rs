//! Exhaustive coverage for the bounded context-usage tone policy.
//!
//! Port parity with `modules/frontend/src/lib/context-usage/gauge-tone.ts`:
//! exact ratio/token normalisation, threshold inclusivity, over-capacity,
//! zero/unknown capacity, non-finite determinism, and display/accessibility
//! rounding. Tests are written against values, never against the implementation
//! re-deriving the expected result.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::float_cmp
)]

use artisan_frontend::context_usage_tone::{
    ContextTone, DANGER_FROM, GaugeToneMix, WARN_FROM, context_gauge_tone_mix,
    context_usage_percent, context_usage_percent_opt, gauge_display_percent, gauge_fill_percent,
    gauge_label_percent, gauge_tone_mix,
};

// ---------------------------------------------------------------------------
// Helpers: JS-faithful oracle for parity checks
// ---------------------------------------------------------------------------

fn js_ramp(from: f64, to: f64, percent: f64) -> u8 {
    if to <= from {
        if percent >= to { 100 } else { 0 }
    } else {
        let raw = (percent - from) / (to - from) * 100.0;
        raw.clamp(0.0, 100.0).round().clamp(0.0, 100.0) as u8
    }
}

fn js_tone(percent: f64, compaction: f64) -> GaugeToneMix {
    let effective = compaction.max(DANGER_FROM);
    GaugeToneMix {
        warn: js_ramp(WARN_FROM, DANGER_FROM, percent),
        danger: js_ramp(DANGER_FROM, effective, percent),
    }
}

fn assert_tone(percent: f64, compaction: f64, expected_warn: u8, expected_danger: u8) {
    let got = context_gauge_tone_mix(percent, compaction);
    assert_eq!(
        got.warn, expected_warn,
        "warn leg at percent={percent} compaction={compaction}"
    );
    assert_eq!(
        got.danger, expected_danger,
        "danger leg at percent={percent} compaction={compaction}"
    );
    // Alias must stay in lockstep.
    assert_eq!(gauge_tone_mix(percent, compaction), got);
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[test]
fn constants_match_typescript_anchors() {
    assert_eq!(WARN_FROM, 50.0);
    assert_eq!(DANGER_FROM, 80.0);
}

// ---------------------------------------------------------------------------
// Ratio / token normalisation
// ---------------------------------------------------------------------------

#[test]
fn percent_ratio_is_clamped_and_finite_safe() {
    // Exact ratios.
    assert_eq!(context_usage_percent(0.0, 100_000.0), Some(0.0));
    assert_eq!(context_usage_percent(50_000.0, 100_000.0), Some(50.0));
    assert_eq!(context_usage_percent(80_000.0, 100_000.0), Some(80.0));
    assert_eq!(context_usage_percent(90_000.0, 100_000.0), Some(90.0));
    // Over-capacity saturates at 100 (Math.min(100, ...)).
    assert_eq!(context_usage_percent(150_000.0, 100_000.0), Some(100.0));
    assert_eq!(context_usage_percent(1_000_000.0, 100_000.0), Some(100.0));
    // Tiny window, large tokens still 100.
    assert_eq!(context_usage_percent(200.0, 1.0), Some(100.0));
    // Negative used clamps to 0.
    assert_eq!(context_usage_percent(-10.0, 100_000.0), Some(0.0));
    // Fractional.
    let pct = context_usage_percent(1.0, 3.0).expect("fractional");
    assert!((pct - 33.333_333_333).abs() < 1e-6);
}

#[test]
fn percent_zero_capacity_is_unknown() {
    assert_eq!(context_usage_percent(10.0, 0.0), None);
    assert_eq!(context_usage_percent(10.0, -1.0), None);
    assert_eq!(context_usage_percent(0.0, 0.0), None);
    assert_eq!(context_usage_percent_opt(Some(10.0), None), None);
    assert_eq!(context_usage_percent_opt(None, Some(100.0)), None);
    assert_eq!(context_usage_percent_opt(None, None), None);
    assert_eq!(context_usage_percent_opt(Some(10.0), Some(0.0)), None);
}

#[test]
fn percent_non_finite_is_unknown() {
    assert_eq!(context_usage_percent(f64::NAN, 100.0), None);
    assert_eq!(context_usage_percent(10.0, f64::NAN), None);
    assert_eq!(context_usage_percent(f64::INFINITY, 100.0), None);
    assert_eq!(context_usage_percent(10.0, f64::INFINITY), None);
    assert_eq!(context_usage_percent(f64::NEG_INFINITY, 100.0), None);
    assert_eq!(context_usage_percent(10.0, f64::NEG_INFINITY), None);
    // Division overflow to inf -> None.
    assert_eq!(context_usage_percent(1e308, 1e-308), None);
}

#[test]
fn percent_opt_preserves_none_semantics() {
    assert_eq!(
        context_usage_percent_opt(Some(50_000.0), Some(100_000.0)),
        Some(50.0)
    );
    assert_eq!(
        context_usage_percent_opt(Some(0.0), Some(100_000.0)),
        Some(0.0)
    );
}

// ---------------------------------------------------------------------------
// Display / accessibility decisions
// ---------------------------------------------------------------------------

#[test]
fn gauge_fill_clamps_directly_from_typescript() {
    // context-usage-details.svelte:34 Math.min(100, Math.max(0, percent))
    assert_eq!(gauge_fill_percent(50.0), 50.0);
    assert_eq!(gauge_fill_percent(-10.0), 0.0);
    assert_eq!(gauge_fill_percent(120.0), 100.0);
    assert_eq!(gauge_fill_percent(0.0), 0.0);
    assert_eq!(gauge_fill_percent(100.0), 100.0);
    assert_eq!(gauge_fill_percent(f64::NAN), 0.0);
    assert_eq!(gauge_fill_percent(f64::INFINITY), 0.0);
    assert_eq!(gauge_fill_percent(f64::NEG_INFINITY), 0.0);
}

#[test]
fn gauge_label_rounds_and_clamps_for_aria() {
    // aria-label Math.round(percent)
    assert_eq!(gauge_label_percent(0.0), 0);
    assert_eq!(gauge_label_percent(0.4), 0);
    assert_eq!(gauge_label_percent(0.5), 1);
    assert_eq!(gauge_label_percent(49.5), 50);
    assert_eq!(gauge_label_percent(74.35), 74);
    assert_eq!(gauge_label_percent(74.5), 75);
    assert_eq!(gauge_label_percent(99.6), 100);
    assert_eq!(gauge_label_percent(120.0), 100);
    assert_eq!(gauge_label_percent(-5.0), 0);
    assert_eq!(gauge_label_percent(f64::NAN), 0);
    assert_eq!(gauge_label_percent(f64::INFINITY), 0);
    assert_eq!(gauge_display_percent(74.5), gauge_label_percent(74.5));
}

#[test]
fn gauge_label_parity_with_js_math_round() {
    let cases: [f64; 10] = [-10.0, 0.0, 49.4, 49.5, 50.0, 79.5, 80.0, 99.5, 100.0, 120.0];
    for pct in cases {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let expected = pct.round().clamp(0.0, 100.0) as u8;
        assert_eq!(gauge_label_percent(pct), expected, "pct={pct}");
    }
}

// ---------------------------------------------------------------------------
// Tone ramp - threshold inclusivity and shape
// ---------------------------------------------------------------------------

#[test]
fn warn_leg_is_zero_below_fifty_and_inclusive_above() {
    assert_tone(0.0, 90.0, 0, 0);
    assert_tone(49.0, 90.0, 0, 0);
    assert_tone(49.9, 90.0, 0, 0);
    assert_tone(50.0, 90.0, 0, 0);
    // Just over 50 warms: (50.5-50)/30*100 = 1.666 -> 2
    assert_tone(50.5, 90.0, 2, 0);
    assert_tone(65.0, 90.0, 50, 0);
    assert_tone(80.0, 90.0, 100, 0);
    assert_tone(100.0, 90.0, 100, 100);
}

#[test]
fn danger_leg_endpoint_tracks_compaction() {
    // compaction 90: 80->90
    assert_tone(80.0, 90.0, 100, 0);
    assert_tone(85.0, 90.0, 100, 50);
    assert_tone(90.0, 90.0, 100, 100);
    assert_tone(95.0, 90.0, 100, 100);
    // compaction 100: 80->100
    assert_tone(80.0, 100.0, 100, 0);
    assert_tone(90.0, 100.0, 100, 50);
    assert_tone(100.0, 100.0, 100, 100);
}

#[test]
fn danger_collapses_to_step_when_compaction_at_or_below_eighty() {
    for compaction in [80.0, 79.0, 50.0, 0.0, -10.0] {
        assert_tone(79.9, compaction, 100, 0);
        assert_tone(80.0, compaction, 100, 100);
        assert_tone(80.1, compaction, 100, 100);
        assert_tone(0.0, compaction, 0, 0);
        assert_tone(100.0, compaction, 100, 100);
    }
}

#[test]
fn thresholds_are_inclusive_exact_boundaries() {
    // At exactly 50 warn=0; at 50.0001 warn>0 not asserted strictly but danger still 0
    let at_50 = context_gauge_tone_mix(50.0, 90.0);
    assert_eq!(at_50.warn, 0);
    assert_eq!(at_50.danger, 0);
    let at_80 = context_gauge_tone_mix(80.0, 90.0);
    assert_eq!(at_80.warn, 100);
    assert_eq!(at_80.danger, 0);
    // Danger leg inclusive at its endpoint 90
    let at_90 = context_gauge_tone_mix(90.0, 90.0);
    assert_eq!(at_90.danger, 100);
    // Step case at 80 inclusive
    let step_at_80 = context_gauge_tone_mix(80.0, 80.0);
    assert_eq!(step_at_80.danger, 100);
    let step_below = context_gauge_tone_mix(79.999, 80.0);
    assert_eq!(step_below.danger, 0);
}

#[test]
fn threshold_adjacent_rounding_matches_typescript_math_round() {
    // (percent - 50)/30*100 at 50.49 -> 1.633 -> 2, at 50.14 -> 0.466 -> 0
    assert_tone(50.14, 90.0, 0, 0);
    assert_tone(50.16, 90.0, 1, 0);
    // Danger leg 80->90 at 80.5 -> 5% -> 5
    assert_tone(80.5, 90.0, 100, 5);
    // 80->100 at 80.5 -> 2.5% -> 3 (JS round tie .5 up)
    assert_tone(80.5, 100.0, 100, 3);
    // Over-capacity beyond compaction stays 100
    assert_tone(120.0, 90.0, 100, 100);
    assert_tone(200.0, 100.0, 100, 100);
}

// ---------------------------------------------------------------------------
// Over-capacity and clamping
// ---------------------------------------------------------------------------

#[test]
fn over_capacity_tones_saturate() {
    assert_tone(120.0, 90.0, 100, 100);
    assert_tone(250.0, 90.0, 100, 100);
    assert_tone(1000.0, 80.0, 100, 100);
    // Negative percent is calm
    assert_tone(-10.0, 90.0, 0, 0);
    assert_tone(-0.1, 100.0, 0, 0);
}

// ---------------------------------------------------------------------------
// Non-finite
// ---------------------------------------------------------------------------

#[test]
fn non_finite_percent_is_calm_and_finite_safe() {
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let tone = context_gauge_tone_mix(bad, 90.0);
        assert_eq!(
            tone,
            GaugeToneMix { warn: 0, danger: 0 },
            "bad percent {bad}"
        );
        // label/fill also safe
        assert_eq!(gauge_label_percent(bad), 0);
        assert_eq!(gauge_fill_percent(bad), 0.0);
        assert_eq!(ContextTone::from_percent(bad), ContextTone::Calm);
    }
}

#[test]
fn non_finite_compaction_falls_back_to_window() {
    // Unknown engine -> 100, so danger 80->100
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert_tone(80.0, bad, 100, 0);
        assert_tone(90.0, bad, 100, 50);
        assert_tone(100.0, bad, 100, 100);
        assert_tone(50.0, bad, 0, 0);
    }
}

#[test]
fn both_non_finite_is_still_calm() {
    let tone = context_gauge_tone_mix(f64::NAN, f64::NAN);
    assert_eq!(tone, GaugeToneMix { warn: 0, danger: 0 });
}

// ---------------------------------------------------------------------------
// Parity with TypeScript over a sweep
// ---------------------------------------------------------------------------

#[test]
fn parity_with_typescript_ramp_over_sweep() {
    let compactions = [80.0, 82.0, 90.0, 100.0, 50.0, 120.0];
    let percents = [
        -10.0, 0.0, 49.0, 49.9, 50.0, 50.5, 65.0, 79.9, 80.0, 80.1, 85.0, 90.0, 99.9, 100.0, 120.0,
    ];
    for compaction in compactions {
        for percent in percents {
            let expected = js_tone(percent, compaction);
            let got = context_gauge_tone_mix(percent, compaction);
            assert_eq!(
                got, expected,
                "parity percent={percent} compaction={compaction}: got {got:?} expected {expected:?}"
            );
        }
    }
}

#[test]
fn parity_fine_grained_rounding_sweep() {
    // 0.1 steps near boundaries to catch any rounding divergence
    for compaction in [90.0, 100.0] {
        let mut pct = 49.0;
        while pct <= 51.0 {
            let expected = js_tone(pct, compaction);
            let got = context_gauge_tone_mix(pct, compaction);
            assert_eq!(got, expected, "warn leg pct {pct} comp {compaction}");
            pct += 0.2;
        }
        let mut pct2 = 79.0;
        while pct2 <= 91.0 {
            let expected = js_tone(pct2, compaction);
            let got = context_gauge_tone_mix(pct2, compaction);
            assert_eq!(got, expected, "danger leg pct {pct2} comp {compaction}");
            pct2 += 0.2;
        }
    }
}

// ---------------------------------------------------------------------------
// Discrete tone names
// ---------------------------------------------------------------------------

#[test]
fn discrete_tone_names_track_thresholds() {
    assert_eq!(ContextTone::from_percent(0.0), ContextTone::Calm);
    assert_eq!(ContextTone::from_percent(49.9), ContextTone::Calm);
    assert_eq!(ContextTone::from_percent(50.0), ContextTone::Warning);
    assert_eq!(ContextTone::from_percent(79.9), ContextTone::Warning);
    assert_eq!(ContextTone::from_percent(80.0), ContextTone::Danger);
    assert_eq!(ContextTone::from_percent(100.0), ContextTone::Danger);
    assert_eq!(ContextTone::from_percent(f64::NAN), ContextTone::Calm);
    assert_eq!(ContextTone::Calm.label(), "calm");
    assert_eq!(ContextTone::Warning.label(), "warning");
    assert_eq!(ContextTone::Danger.label(), "danger");
}

// ---------------------------------------------------------------------------
// End-to-end: token ratio -> tone parity (zero / over-capacity chains)
// ---------------------------------------------------------------------------

#[test]
fn token_ratio_chain_matches_js_percent_then_tone() {
    // Emulate TS: percent = min(100, used/window*100), then ContextGaugeToneMix(percent, compaction)
    let cases: [(f64, f64, f64); 6] = [
        (0.0, 100_000.0, 90.0),
        (50_000.0, 100_000.0, 90.0),
        (80_000.0, 100_000.0, 90.0),
        (95_000.0, 100_000.0, 90.0),
        (150_000.0, 100_000.0, 90.0), // over-capacity -> 100
        (80_000.0, 100_000.0, 100.0),
    ];
    for (used, window, compaction) in cases {
        let pct = context_usage_percent(used, window).expect("has percent");
        let tone = context_gauge_tone_mix(pct, compaction);
        let js_pct = (used / window * 100.0).clamp(0.0, 100.0);
        let expected = js_tone(js_pct, compaction);
        assert_eq!(tone, expected, "chain used {used} window {window}");
    }
}

#[test]
fn unknown_window_produces_no_percent_and_calm_fallback_is_not_a_gauge() {
    // Callers (controls.svelte) show no gauge when percent is None; they must
    // not synthesize a 0% calm ring as if it were a reading.
    assert_eq!(context_usage_percent(10_000.0, 0.0), None);
    assert_eq!(context_usage_percent(f64::NAN, 100.0), None);
    // If a caller did force a 0 tone for unknown, it would be calm {0,0} but
    // the correct behavior is to render no gauge at all — tested via Option.
    let unknown = context_usage_percent_opt(None, Some(100_000.0));
    assert_eq!(unknown, None);
}
