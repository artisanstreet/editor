#![expect(
    clippy::assigning_clones,
    clippy::cast_precision_loss,
    clippy::clone_on_ref_ptr,
    clippy::collapsible_if,
    clippy::large_types_passed_by_value,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::match_bool,
    clippy::match_same_arms,
    clippy::needless_borrow,
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    clippy::option_option,
    clippy::or_fun_call,
    clippy::partial_pub_fields,
    clippy::redundant_closure_for_method_calls,
    clippy::semicolon_if_nothing_returned,
    clippy::similar_names,
    clippy::struct_excessive_bools,
    clippy::suboptimal_flops,
    clippy::too_many_lines,
    clippy::unnecessary_to_owned,
    clippy::unwrap_used,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    missing_debug_implementations,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
extern crate self as editor_settings;

mod settings;
pub use settings::*;
