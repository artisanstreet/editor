//! Native Artisan Editor application assembly boundary.
//!
//! Phase 1 feasibility state: this crate currently launches the minimal GPUI
//! proof window so the upstream native toolchain can be exercised end to end.
//! Product assembly, navigation, and screens arrive in later phases; nothing
//! here is Artisan visual or interaction design.

pub mod proof;

use std::process::ExitCode;

/// Runs the currently implemented native editor boundary.
#[must_use]
pub fn run() -> ExitCode {
    proof::run()
}
