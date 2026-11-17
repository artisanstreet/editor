#![expect(
    clippy::string_slice,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![cfg(test)]

#[expect(
    clippy::disallowed_types,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "inspector/mod.rs"]
mod inspector;
