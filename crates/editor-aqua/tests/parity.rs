#![expect(
    clippy::assertions_on_result_states,
    clippy::wildcard_enum_match_arm,
    unreachable_pub,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "parity/mod.rs"]
mod parity;
