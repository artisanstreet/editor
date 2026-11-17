#![expect(
    clippy::string_slice,
    clippy::unwrap_used,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "sources/mod.rs"]
mod sources;
