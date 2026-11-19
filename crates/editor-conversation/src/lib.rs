#![expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
extern crate self as editor_conversation;

mod conversation;
pub use conversation::*;
