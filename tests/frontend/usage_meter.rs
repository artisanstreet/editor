//! Boundary and table coverage for the native provider-usage meter.
//!
//! The cases mirror the sidebar contract: 14 ticks, a zero reading, visible
//! positive readings, exact and adjacent boundaries, clamping, and finite-safe
//! handling of malformed provider values.

#![allow(clippy::float_cmp)]

use artisan_frontend::usage_meter::{
    USAGE_METER_SEGMENTS, usage_meter_segments, usage_segment_fraction,
};

fn expected_fraction(ticks: u8) -> f64 {
    f64::from(ticks) / f64::from(USAGE_METER_SEGMENTS)
}

fn tick_boundary(tick: u8) -> f64 {
    f64::from(tick) * 100.0 / f64::from(USAGE_METER_SEGMENTS)
}

fn next_representable(value: f64) -> f64 {
    f64::from_bits(value.to_bits() + 1)
}

#[test]
fn constants_expose_the_fourteen_tick_contract() {
    assert_eq!(USAGE_METER_SEGMENTS, 14);
    assert_eq!(usage_meter_segments, USAGE_METER_SEGMENTS);
}

#[test]
fn quantization_table_preserves_zero_visibility_and_clamping() {
    let table = [
        (0.0, 0),
        (f64::MIN_POSITIVE, 1),
        (1.0, 1),
        (50.0, 7),
        (100.0, 14),
        (100.000_001, 14),
        (f64::MAX, 14),
        (-0.000_001, 0),
        (-100.0, 0),
        (-f64::MAX, 0),
    ];

    for (percent_used, ticks) in table {
        assert_eq!(
            usage_segment_fraction(percent_used),
            expected_fraction(ticks),
            "percent_used={percent_used}"
        );
    }
}

#[test]
fn every_exact_tick_boundary_returns_its_exact_segment_fraction() {
    for tick in 1..USAGE_METER_SEGMENTS {
        let boundary = tick_boundary(tick);
        assert_eq!(
            usage_segment_fraction(boundary),
            expected_fraction(tick),
            "exact boundary tick={tick}, percent_used={boundary}"
        );
    }
}

#[test]
fn every_just_over_boundary_advances_by_one_tick() {
    for tick in 1..USAGE_METER_SEGMENTS {
        let boundary = tick_boundary(tick);
        let just_over = next_representable(boundary);
        assert_eq!(
            usage_segment_fraction(just_over),
            expected_fraction(tick + 1),
            "just over tick={tick}, percent_used={just_over}"
        );
    }
}

#[test]
fn non_finite_readings_are_finite_and_unlit() {
    for percent_used in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let fraction = usage_segment_fraction(percent_used);
        assert!(
            fraction.is_finite(),
            "fraction for {percent_used} is finite"
        );
        assert_eq!(fraction, 0.0, "malformed reading {percent_used}");
    }
}
