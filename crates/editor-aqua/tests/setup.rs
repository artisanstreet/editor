#![expect(
    clippy::cast_possible_truncation,
    clippy::suboptimal_flops,
    clippy::wildcard_enum_match_arm,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "setup/model.rs"]
mod model;
#[path = "setup/view.rs"]
mod view;
