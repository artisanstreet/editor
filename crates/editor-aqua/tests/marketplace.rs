#![expect(
    clippy::clone_on_ref_ptr,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "marketplace/mod.rs"]
mod marketplace;
