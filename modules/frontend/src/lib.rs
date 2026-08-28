//! Native Artisan Editor application assembly boundary.
//!
//! Phase 1 feasibility state: the binary launches the minimal GPUI proof
//! window so the upstream native toolchain can be exercised end to end.
//! Product assembly, navigation, and screens arrive in later phases. Beyond
//! that window, the library now hosts narrow product presentation-model
//! leaves that record audited semantics without rendering them:
//! [`transcript`] and [`thread_list_selection`].

pub mod proof;
pub mod thread_list_selection;
pub mod transcript;

use std::process::ExitCode;

/// Runs the currently implemented native editor boundary.
#[must_use]
pub fn run() -> ExitCode {
    proof::run()
}
