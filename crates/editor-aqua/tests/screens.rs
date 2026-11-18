#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::derive_partial_eq_without_eq,
    clippy::print_stdout,
    unreachable_pub,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "qualification/diff.rs"]
mod diff;
#[path = "qualification/screens.rs"]
mod screens;
