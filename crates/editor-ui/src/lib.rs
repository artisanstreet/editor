#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "editor-ui is the kit: it builds the components and tokens that these bans steer every other crate to"
)]
extern crate self as editor_ui;

mod ui;
pub use ui::*;
