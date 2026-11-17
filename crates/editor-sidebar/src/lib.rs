#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::clone_on_ref_ptr,
    clippy::collapsible_if,
    clippy::field_scoped_visibility_modifiers,
    clippy::imprecise_flops,
    clippy::large_types_passed_by_value,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::match_bool,
    clippy::match_same_arms,
    clippy::match_wildcard_for_single_variants,
    clippy::needless_pass_by_value,
    clippy::option_option,
    clippy::partial_pub_fields,
    clippy::redundant_clone,
    clippy::redundant_closure_for_method_calls,
    clippy::semicolon_if_nothing_returned,
    clippy::string_slice,
    clippy::struct_excessive_bools,
    clippy::suboptimal_flops,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::unused_result_ok,
    clippy::unwrap_used,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    missing_debug_implementations,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::future_not_send,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
extern crate self as editor_sidebar;

mod sidebar;
pub use sidebar::*;
