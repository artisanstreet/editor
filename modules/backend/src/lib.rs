//! Forge application assembly boundary.

use std::process::ExitCode;

/// Runs the currently implemented Forge process boundary.
#[must_use]
pub const fn run() -> ExitCode {
    ExitCode::SUCCESS
}
