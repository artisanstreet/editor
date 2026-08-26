//! Native Artisan Editor application assembly boundary.
//!
//! The binary launches the minimal GPUI proof window, now embedding the
//! product-specific [`project_picker`] leaf. Complete product assembly,
//! navigation, and screens remain later work. Beyond that window, the
//! library hosts narrow product-state models without
//! rendering them: [`attention`], [`composer`], [`transcript`], and
//! [`thread_list_selection`].

pub mod attention;
pub mod composer;
pub mod conversation_projection;
pub mod project_picker;
pub mod proof;
pub mod thread_list_selection;
pub mod transcript;

use std::process::ExitCode;

/// Runs the currently implemented native editor boundary.
#[must_use]
pub fn run() -> ExitCode {
    proof::run()
}
