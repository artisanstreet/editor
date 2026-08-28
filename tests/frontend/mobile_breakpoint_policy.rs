//! Dependency-free boundary coverage for the native mobile breakpoint policy.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/mobile_breakpoint_policy.rs"]
mod mobile_breakpoint_policy;

use mobile_breakpoint_policy::{
    DEFAULT_MOBILE_BREAKPOINT_PIXELS, MobileBreakpoint, MobileBreakpointError,
    MobileBreakpointPolicy,
};

#[test]
fn default_policy_preserves_the_legacy_768_pixel_boundary() {
    let policy = MobileBreakpointPolicy::default();

    assert_eq!(DEFAULT_MOBILE_BREAKPOINT_PIXELS, 768);
    assert_eq!(policy.breakpoint_pixels(), 768);
    assert_eq!(policy.mobile_max_width(), 767);
    assert_eq!(policy.media_query(), "(max-width: 767px)");
    assert_eq!(policy.breakpoint().inclusive_mobile_max(), 767);
}

#[test]
fn zero_is_rejected_by_the_breakpoint_and_policy() {
    let breakpoint_error = MobileBreakpoint::new(0).expect_err("zero must be rejected");
    let policy_error =
        MobileBreakpointPolicy::new(0).expect_err("zero must be rejected by the policy");

    assert_eq!(breakpoint_error, MobileBreakpointError::Zero);
    assert_eq!(policy_error, MobileBreakpointError::Zero);
    assert_eq!(
        breakpoint_error.to_string(),
        "mobile breakpoint must be greater than zero"
    );
}

#[test]
fn every_positive_custom_breakpoint_is_retained_without_clamping() {
    for pixels in [1, 2, 320, 768, u32::MAX] {
        let breakpoint = MobileBreakpoint::new(pixels).expect("positive breakpoint validates");
        let policy = MobileBreakpointPolicy::from_breakpoint(breakpoint);

        assert_eq!(breakpoint.pixels(), pixels);
        assert_eq!(policy.breakpoint_pixels(), pixels);
        assert_eq!(policy.mobile_max_width(), pixels - 1);
    }
}

#[test]
fn default_classification_matches_the_inclusive_query_boundary() {
    let policy = MobileBreakpointPolicy::default();
    let mobile_max = policy.mobile_max_width();

    for (width, expected_mobile) in [(0, true), (767, true), (768, false), (u32::MAX, false)] {
        assert_eq!(policy.is_mobile(width), expected_mobile, "width {width}");
        assert_eq!(
            policy.is_mobile(width),
            width <= mobile_max,
            "width {width} must agree with the inclusive query"
        );
    }
}

#[test]
fn custom_classification_matches_at_lower_boundary_and_u32_max() {
    for breakpoint_pixels in [1, 2, 320, u32::MAX] {
        let policy = MobileBreakpointPolicy::new(breakpoint_pixels)
            .expect("positive custom breakpoint validates");
        let mobile_max = policy.mobile_max_width();

        for (width, expected_mobile) in [
            (0, 0 < breakpoint_pixels),
            (mobile_max, true),
            (breakpoint_pixels, false),
            (u32::MAX, false),
        ] {
            assert_eq!(policy.is_mobile(width), expected_mobile, "width {width}");
            assert_eq!(
                policy.is_mobile(width),
                width <= mobile_max,
                "width {width} must agree with the inclusive query"
            );
        }
    }
}

#[test]
fn media_query_formatting_is_exact_for_default_small_and_maximum_values() {
    let cases = [
        (1, "(max-width: 0px)"),
        (320, "(max-width: 319px)"),
        (768, "(max-width: 767px)"),
        (u32::MAX, "(max-width: 4294967294px)"),
    ];

    for (breakpoint_pixels, expected_query) in cases {
        let policy =
            MobileBreakpointPolicy::new(breakpoint_pixels).expect("positive breakpoint validates");
        assert_eq!(policy.media_query(), expected_query);
    }
}

#[test]
fn validated_breakpoint_ordering_and_default_values_are_stable() {
    let default_breakpoint = MobileBreakpoint::default();
    let custom_breakpoint =
        MobileBreakpoint::new(DEFAULT_MOBILE_BREAKPOINT_PIXELS).expect("default validates");

    assert_eq!(default_breakpoint, custom_breakpoint);
    assert_eq!(
        default_breakpoint.pixels(),
        DEFAULT_MOBILE_BREAKPOINT_PIXELS
    );
    assert!(MobileBreakpoint::new(1).expect("one validates") < default_breakpoint);
    assert!(MobileBreakpoint::new(u32::MAX).expect("maximum validates") > default_breakpoint);
}
