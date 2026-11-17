#![expect(
    clippy::clone_on_ref_ptr,
    clippy::float_cmp,
    clippy::semicolon_if_nothing_returned,
    clippy::wildcard_enum_match_arm,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[expect(
    clippy::disallowed_types,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "text/mod.rs"]
mod text;
#[path = "text/wheel_glide.rs"]
mod wheel_glide;
