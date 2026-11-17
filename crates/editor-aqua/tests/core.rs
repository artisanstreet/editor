#![expect(
    clippy::assertions_on_result_states,
    clippy::clone_on_ref_ptr,
    clippy::collapsible_if,
    clippy::default_trait_access,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::too_many_lines,
    clippy::unwrap_used,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "core/dial.rs"]
mod dial;
#[path = "core/open_timing.rs"]
mod open_timing;
#[path = "core/ordering.rs"]
mod ordering;
#[path = "core/stopping.rs"]
mod stopping;

#[path = "core/account.rs"]
mod account;
