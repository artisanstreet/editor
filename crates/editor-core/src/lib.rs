#![expect(
    clippy::allow_attributes,
    clippy::clone_on_ref_ptr,
    clippy::collapsible_if,
    clippy::default_trait_access,
    clippy::duration_suboptimal_units,
    clippy::explicit_into_iter_loop,
    clippy::field_scoped_visibility_modifiers,
    clippy::fn_params_excessive_bools,
    clippy::items_after_statements,
    clippy::iter_over_hash_type,
    clippy::large_enum_variant,
    clippy::let_and_return,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::manual_let_else,
    clippy::match_same_arms,
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    clippy::nonminimal_bool,
    clippy::partial_pub_fields,
    clippy::print_stderr,
    clippy::rc_buffer,
    clippy::redundant_closure_for_method_calls,
    clippy::renamed_function_params,
    clippy::semicolon_if_nothing_returned,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::string_slice,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::unnested_or_patterns,
    clippy::unused_result_ok,
    clippy::unwrap_used,
    clippy::use_self,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    missing_debug_implementations,
    unreachable_pub,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::used_underscore_binding,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
extern crate self as editor_core;

mod core;
pub use core::*;
