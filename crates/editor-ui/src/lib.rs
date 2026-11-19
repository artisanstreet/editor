#![expect(
    clippy::large_types_passed_by_value,
    clippy::needless_pass_by_value,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::unwrap_used,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "editor-ui is the kit: it builds the components and tokens that these bans steer every other crate to"
)]
extern crate self as editor_ui;

mod ui;
pub use ui::*;
