#![expect(
    clippy::float_cmp,
    clippy::redundant_clone,
    clippy::suboptimal_flops,
    clippy::too_many_lines,
    clippy::unchecked_time_subtraction,
    clippy::unwrap_used,
    clippy::while_float,
    clippy::wildcard_enum_match_arm,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "frame/glide.rs"]
mod glide;
#[path = "frame/menu.rs"]
mod menu;
#[path = "frame/scramble.rs"]
mod scramble;
#[path = "frame/sign_in/dither.rs"]
mod sign_in_dither;
#[path = "frame/sign_in/model.rs"]
mod sign_in_model;
#[path = "frame/sign_in/pattern.rs"]
mod sign_in_pattern;
#[path = "frame/strip.rs"]
mod strip;
#[path = "frame/wordmark.rs"]
mod wordmark;

#[path = "frame/account.rs"]
mod account;
#[path = "frame/limits.rs"]
mod limits;

#[path = "frame/seam/outline.rs"]
mod seam_outline;

#[path = "frame/seam/motion.rs"]
mod seam_motion;

#[path = "frame/sidebar.rs"]
mod sidebar;

#[path = "frame/inbox.rs"]
mod inbox;
