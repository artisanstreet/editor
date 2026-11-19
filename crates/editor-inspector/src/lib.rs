#![allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
extern crate self as editor_inspector;

mod inspector;
pub use inspector::*;
