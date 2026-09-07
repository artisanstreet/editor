//! Direct, dependency-free parity coverage for the hover-pill geometry policy.
//!
//! The production module is included directly so this focused harness can be
//! compiled with pinned Rust 1.98 without Cargo, Bazel, or frontend
//! registration changes.

#![allow(clippy::float_cmp)]

#[path = "../../modules/frontend/src/hover_pill_geometry_policy.rs"]
mod hover_pill_geometry_policy;

use hover_pill_geometry_policy::{
    ClientRect, HoverPillGeometry, HoverPillGeometryInput, HoverPillPresentation,
    compute_hover_pill_geometry, hover_pill_presentation,
};

fn rect(left: f64, top: f64) -> ClientRect {
    ClientRect::new(left, top)
}

fn input(
    target_rect: Option<ClientRect>,
    anchor_rect: Option<ClientRect>,
    width: f64,
    height: f64,
    animated: bool,
    measurement_version: u64,
) -> HoverPillGeometryInput {
    HoverPillGeometryInput::new(
        target_rect,
        anchor_rect,
        width,
        height,
        animated,
        measurement_version,
    )
}

fn expected_presentation(
    geometry: Option<HoverPillGeometry>,
    active: bool,
    animate: bool,
    measurement_version: u64,
) -> HoverPillPresentation {
    HoverPillPresentation {
        active,
        animate,
        geometry,
        measurement_version,
    }
}

#[test]
fn missing_target_or_anchor_is_inactive_without_geometry() {
    let cases = [
        (None, Some(rect(10.0, 20.0))),
        (Some(rect(30.0, 40.0)), None),
        (None, None),
    ];

    for (target, anchor) in cases {
        let presentation = hover_pill_presentation(input(target, anchor, 120.0, 32.0, true, 7));

        assert_eq!(
            presentation,
            expected_presentation(None, false, true, 7),
            "target={target:?}, anchor={anchor:?}"
        );
        assert!(!presentation.is_active());
        assert!(presentation.should_animate());
        assert_eq!(
            compute_hover_pill_geometry(target, anchor, 120.0, 32.0),
            None
        );
    }
}

#[test]
fn client_rect_differences_preserve_exact_negative_and_fractional_coordinates() {
    let presentation = hover_pill_presentation(input(
        Some(rect(-12.25, 48.875)),
        Some(rect(7.5, 100.125)),
        164.0,
        29.0,
        false,
        3,
    ));

    assert_eq!(
        presentation,
        expected_presentation(
            Some(HoverPillGeometry::new(-19.75, -51.25, 164.0, 29.0)),
            true,
            false,
            3,
        )
    );
}

#[test]
fn dimensions_are_taken_directly_from_target_offsets() {
    let geometry = compute_hover_pill_geometry(
        Some(rect(300.0, 220.0)),
        Some(rect(275.0, 200.0)),
        0.0,
        513.0,
    )
    .expect("both measured elements produce geometry");

    assert_eq!(geometry.left, 25.0);
    assert_eq!(geometry.top, 20.0);
    assert_eq!(geometry.width, 0.0);
    assert_eq!(geometry.height, 513.0);
}

#[test]
fn animation_flag_is_exposed_independently_of_geometry() {
    let base = input(
        Some(rect(50.0, 75.0)),
        Some(rect(10.0, 25.0)),
        80.0,
        24.0,
        false,
        11,
    );
    let animated = hover_pill_presentation(HoverPillGeometryInput {
        animated: true,
        ..base
    });
    let immediate = hover_pill_presentation(base);

    assert_eq!(animated.geometry, immediate.geometry);
    assert_eq!(animated.active, immediate.active);
    assert!(animated.should_animate());
    assert!(!immediate.should_animate());

    let inactive = hover_pill_presentation(input(None, None, 80.0, 24.0, false, 12));
    assert!(!inactive.active);
    assert!(!inactive.should_animate());
}

#[test]
fn a_new_measurement_version_recomputes_equivalent_geometry_without_rewriting_it() {
    let measurements = |measurement_version| {
        hover_pill_presentation(input(
            Some(rect(123.125, -45.5)),
            Some(rect(100.0, -60.25)),
            96.0,
            28.0,
            true,
            measurement_version,
        ))
    };

    let first = measurements(4);
    let remeasured = measurements(5);

    assert_eq!(first.geometry, remeasured.geometry);
    assert_eq!(first.active, remeasured.active);
    assert_eq!(first.animate, remeasured.animate);
    assert_eq!(first.measurement_version, 4);
    assert_eq!(remeasured.measurement_version, 5);
    assert_ne!(first.measurement_version, remeasured.measurement_version);
}
