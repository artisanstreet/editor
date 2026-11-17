#![expect(
    clippy::assigning_clones,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::clone_on_copy,
    clippy::clone_on_ref_ptr,
    clippy::collapsible_if,
    clippy::collapsible_match,
    clippy::default_trait_access,
    clippy::field_scoped_visibility_modifiers,
    clippy::float_cmp,
    clippy::fn_params_excessive_bools,
    clippy::if_not_else,
    clippy::if_then_some_else_none,
    clippy::inconsistent_struct_constructor,
    clippy::items_after_statements,
    clippy::iter_over_hash_type,
    clippy::large_types_passed_by_value,
    clippy::let_and_return,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::map_err_ignore,
    clippy::map_unwrap_or,
    clippy::match_bool,
    clippy::match_same_arms,
    clippy::match_wildcard_for_single_variants,
    clippy::needless_borrow,
    clippy::needless_borrows_for_generic_args,
    clippy::needless_continue,
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    clippy::nonminimal_bool,
    clippy::option_option,
    clippy::or_fun_call,
    clippy::partial_pub_fields,
    clippy::print_stderr,
    clippy::redundant_closure_for_method_calls,
    clippy::renamed_function_params,
    clippy::return_self_not_must_use,
    clippy::semicolon_if_nothing_returned,
    clippy::similar_names,
    clippy::single_match_else,
    clippy::string_slice,
    clippy::struct_excessive_bools,
    clippy::suboptimal_flops,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::unnecessary_fold,
    clippy::unnecessary_semicolon,
    clippy::unused_self,
    clippy::unwrap_used,
    clippy::used_underscore_binding,
    clippy::wildcard_enum_match_arm,
    dead_code,
    let_underscore_drop,
    missing_debug_implementations,
    unreachable_pub,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[cfg(feature = "capture")]
pub mod capture;
pub use editor_composer as composer;
pub use editor_conversation as conversation;
pub use editor_core as core;
pub mod frame;
pub use editor_inspector as inspector;
pub mod logging;
pub use editor_marketplace as marketplace;
#[cfg(feature = "qualification")]
pub mod qualification;
pub use editor_settings as settings;
pub mod setup;
pub mod shell;
pub use editor_sidebar as sidebar;
pub use editor_sources as sources;
pub use editor_ui as ui;

mod launch;
pub use launch::launch;
