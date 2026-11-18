#![expect(
    clippy::collapsible_if,
    clippy::derivable_impls,
    clippy::large_types_passed_by_value,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::match_bool,
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    clippy::partial_pub_fields,
    clippy::redundant_clone,
    clippy::redundant_closure_for_method_calls,
    clippy::renamed_function_params,
    clippy::semicolon_if_nothing_returned,
    clippy::single_match_else,
    clippy::string_slice,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    missing_debug_implementations,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
extern crate self as editor_inspector;

mod inspector;
pub use inspector::*;
