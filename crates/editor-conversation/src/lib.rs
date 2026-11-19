#![expect(
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::fn_params_excessive_bools,
    clippy::implicit_hasher,
    clippy::iter_over_hash_type,
    clippy::large_enum_variant,
    clippy::large_types_passed_by_value,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::match_same_arms,
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    clippy::partial_pub_fields,
    clippy::significant_drop_in_scrutinee,
    clippy::similar_names,
    clippy::string_slice,
    clippy::struct_excessive_bools,
    clippy::suspicious_operation_groupings,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::type_complexity,
    clippy::unused_self,
    clippy::unwrap_used,
    let_underscore_drop,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
extern crate self as editor_conversation;

mod conversation;
pub use conversation::*;
