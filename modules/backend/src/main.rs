//! Forge process entry point.
//!
//! The internal directory helper mode is dispatched before anything else so
//! it can never reach normal backend, storage, network, or UI startup. When
//! the helper flag is absent this falls through to the ordinary Forge
//! boundary unchanged.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Directory-helper dispatch is deliberately the first normal-process
    // action: helper mode must not construct the Forge runtime or its owners.
    if let Some(exit_code) = artisan_backend::directory_helper::run_if_requested() {
        return exit_code;
    }
    artisan_backend::run()
}
