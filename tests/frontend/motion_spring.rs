//! Numeric parity coverage for the pure motion spring contract.
//!
//! The source is included directly so this focused test remains dependency-
//! free and can be compiled with `rustc --test` before the frontend crate's
//! shared module and test registrations are added.

#[path = "../../modules/frontend/src/motion_spring.rs"]
mod motion_spring;

use motion_spring::{
    DEFAULT_EPSILON, SpringState, spring_settled, spring_settled_with_epsilon, spring_step,
};

const STEP_TOLERANCE: f64 = 1e-12;

fn assert_close(actual: f64, expected: f64, tolerance: f64, label: &str) {
    assert!(
        (actual - expected).abs() <= tolerance,
        "{label}: expected {expected:.17e}, got {actual:.17e}, tolerance {tolerance:.1e}"
    );
}

fn assert_state_close(actual: SpringState, expected: SpringState, tolerance: f64) {
    assert_close(actual.value, expected.value, tolerance, "value");
    assert_close(actual.velocity, expected.velocity, tolerance, "velocity");
}

fn next_lower(value: f64) -> f64 {
    f64::from_bits(value.to_bits() - 1)
}

#[test]
fn stationary_state_stays_stationary() {
    let state = SpringState {
        value: 4.25,
        velocity: 0.0,
    };

    let next = spring_step(state, 4.25, 1.0 / 60.0, 180.0, 24.0);

    assert_eq!(next.value.to_bits(), state.value.to_bits());
    assert_eq!(next.velocity.to_bits(), state.velocity.to_bits());
}

#[test]
fn acceleration_uses_updated_velocity_for_position() {
    let next = spring_step(
        SpringState {
            value: -0.35,
            velocity: 0.45,
        },
        1.2,
        0.016,
        150.0,
        22.0,
    );

    assert_state_close(
        next,
        SpringState {
            value: -0.285_814_4,
            velocity: 4.0116,
        },
        STEP_TOLERANCE,
    );
}

#[test]
fn damping_reduces_an_existing_positive_velocity() {
    let next = spring_step(
        SpringState {
            value: 1.6,
            velocity: 3.75,
        },
        -0.4,
        0.025,
        90.0,
        17.0,
    );

    assert_state_close(
        next,
        SpringState {
            value: 1.541_406_25,
            velocity: -2.34375,
        },
        STEP_TOLERANCE,
    );
}

#[test]
fn retargeting_preserves_incoming_velocity() {
    let next = spring_step(
        SpringState {
            value: 0.82,
            velocity: -2.4,
        },
        -1.35,
        0.0125,
        240.0,
        28.0,
    );

    assert_state_close(
        next,
        SpringState {
            value: 0.719_125,
            velocity: -8.07,
        },
        STEP_TOLERANCE,
    );
}

#[test]
fn settled_requires_strictly_inside_both_epsilon_boundaries() {
    assert_eq!(DEFAULT_EPSILON.to_bits(), 0.0005_f64.to_bits());

    let just_inside = next_lower(DEFAULT_EPSILON);

    assert!(!spring_settled_with_epsilon(
        SpringState {
            value: DEFAULT_EPSILON,
            velocity: 0.0,
        },
        0.0,
        DEFAULT_EPSILON,
    ));
    assert!(!spring_settled_with_epsilon(
        SpringState {
            value: 0.0,
            velocity: -DEFAULT_EPSILON,
        },
        0.0,
        DEFAULT_EPSILON,
    ));
    assert!(spring_settled_with_epsilon(
        SpringState {
            value: just_inside,
            velocity: 0.0,
        },
        0.0,
        DEFAULT_EPSILON,
    ));
    assert!(spring_settled_with_epsilon(
        SpringState {
            value: 0.0,
            velocity: -just_inside,
        },
        0.0,
        DEFAULT_EPSILON,
    ));

    assert!(!spring_settled(
        SpringState {
            value: DEFAULT_EPSILON,
            velocity: 0.0,
        },
        0.0,
    ));
    assert!(spring_settled(
        SpringState {
            value: just_inside,
            velocity: 0.0,
        },
        0.0,
    ));
}

#[test]
fn negative_velocity_is_part_of_the_damping_calculation() {
    let next = spring_step(
        SpringState {
            value: 0.8,
            velocity: -1.25,
        },
        0.0,
        0.02,
        30.0,
        4.3,
    );

    assert_state_close(
        next,
        SpringState {
            value: 0.76755,
            velocity: -1.6225,
        },
        STEP_TOLERANCE,
    );
}

#[test]
fn zero_dt_keeps_value_and_velocity_unchanged() {
    let state = SpringState {
        value: 7.25,
        velocity: -3.5,
    };

    let next = spring_step(state, -11.0, 0.0, 99.0, 12.0);

    assert_eq!(next.value.to_bits(), state.value.to_bits());
    assert_eq!(next.velocity.to_bits(), state.velocity.to_bits());
}

#[test]
fn repeated_steps_follow_the_same_semi_implicit_trajectory() {
    let mut state = SpringState {
        value: 0.0,
        velocity: 0.0,
    };
    let expected = [
        (
            1,
            SpringState {
                value: 0.05,
                velocity: 3.0,
            },
        ),
        (
            2,
            SpringState {
                value: 0.1275,
                velocity: 4.65,
            },
        ),
        (
            3,
            SpringState {
                value: 0.217_625,
                velocity: 5.4075,
            },
        ),
        (
            10,
            SpringState {
                value: 0.739_443_814_528_417_9,
                velocity: 3.063_640_061_431_640_6,
            },
        ),
        (
            30,
            SpringState {
                value: 0.995_583_986_113_368_6,
                velocity: 0.063_561_476_946_921_37,
            },
        ),
        (
            60,
            SpringState {
                value: 0.999_993_965_692_525_4,
                velocity: 0.000_090_036_238_675_482_1,
            },
        ),
        (
            120,
            SpringState {
                value: 0.999_999_999_990_609,
                velocity: 0.000_000_000_140_851_368_248_418,
            },
        ),
    ];

    let mut expected_index = 0;
    for frame in 1..=120 {
        state = spring_step(state, 1.0, 1.0 / 60.0, 180.0, 24.0);
        if expected_index < expected.len() && frame == expected[expected_index].0 {
            assert_state_close(state, expected[expected_index].1, STEP_TOLERANCE);
            expected_index += 1;
        }
    }

    assert_eq!(expected_index, expected.len());
    assert!(spring_settled(state, 1.0));
}
