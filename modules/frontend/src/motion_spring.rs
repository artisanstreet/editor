//! Frame-by-frame damped spring arithmetic.
//!
//! This is the pure Rust counterpart of
//! `modules/frontend/src/lib/motion/spring.ts`. It carries only a scalar
//! value and its velocity; measuring layout, choosing a duration, accumulating
//! time, and rendering remain outside this module.

/// The default distance and speed tolerance used by [`spring_settled`].
pub const DEFAULT_EPSILON: f64 = 0.0005;

/// The position and velocity of a scalar spring at one frame boundary.
///
/// The state is intentionally just the two values needed by the one-frame
/// integrator. Callers retain the state between frames and may change the
/// target without resetting the velocity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpringState {
    /// The spring's current scalar position.
    pub value: f64,
    /// The spring's current scalar velocity.
    pub velocity: f64,
}

/// Advances a spring by one frame using semi-implicit Euler integration.
///
/// The arithmetic order is deliberately the same as the TypeScript source:
/// first compute `(target - value) * stiffness - velocity * damping`, then
/// update velocity, and finally use that updated velocity for the value. No
/// clamping, validation, or elapsed-time accumulation is performed here.
#[must_use]
pub fn spring_step(
    state: SpringState,
    target: f64,
    dt: f64,
    stiffness: f64,
    damping: f64,
) -> SpringState {
    let acceleration = (target - state.value) * stiffness - state.velocity * damping;
    let velocity = state.velocity + acceleration * dt;
    SpringState {
        value: state.value + velocity * dt,
        velocity,
    }
}

/// Returns whether the spring is strictly within a caller-supplied epsilon.
///
/// Both the target distance and the absolute velocity must be strictly less
/// than `epsilon`, matching the TypeScript `Math.abs(...) < epsilon` checks.
/// Equality at either boundary is not settled.
#[must_use]
pub fn spring_settled_with_epsilon(state: SpringState, target: f64, epsilon: f64) -> bool {
    (target - state.value).abs() < epsilon && state.velocity.abs() < epsilon
}

/// Returns whether the spring is settled under the TypeScript default.
///
/// This explicit wrapper supplies [`DEFAULT_EPSILON`] because Rust has no
/// default function arguments.
#[must_use]
pub fn spring_settled(state: SpringState, target: f64) -> bool {
    spring_settled_with_epsilon(state, target, DEFAULT_EPSILON)
}
