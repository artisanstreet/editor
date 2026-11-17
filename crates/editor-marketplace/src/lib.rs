#![expect(
    clippy::cast_precision_loss,
    clippy::clone_on_ref_ptr,
    clippy::collapsible_if,
    clippy::if_then_some_else_none,
    clippy::inconsistent_struct_constructor,
    clippy::large_types_passed_by_value,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    clippy::partial_pub_fields,
    clippy::semicolon_if_nothing_returned,
    clippy::struct_excessive_bools,
    clippy::too_many_lines,
    clippy::unused_self,
    clippy::unwrap_used,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    missing_debug_implementations,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
extern crate self as editor_marketplace;

mod marketplace;
pub use marketplace::*;
