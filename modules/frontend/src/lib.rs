//! Native Artisan Editor application assembly boundary.

use std::process::ExitCode;

/// Runs the currently implemented native editor boundary.
#[must_use]
pub const fn run() -> ExitCode {
    ExitCode::SUCCESS
}
