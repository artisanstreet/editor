#![expect(
    clippy::clone_on_ref_ptr,
    clippy::unwrap_used,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "settings/mod.rs"]
mod settings;
