#![expect(
    clippy::float_cmp,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "ui/rich/charts/distribution.rs"]
mod distribution;
