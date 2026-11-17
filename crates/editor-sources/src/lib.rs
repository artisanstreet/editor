#![expect(
    clippy::large_types_passed_by_value,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::needless_pass_by_ref_mut,
    clippy::needless_pass_by_value,
    clippy::partial_pub_fields,
    clippy::semicolon_if_nothing_returned,
    clippy::string_slice,
    clippy::suspicious_operation_groupings,
    clippy::type_complexity,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    missing_debug_implementations,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::redundant_closure_for_method_calls,
    unused_qualifications,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
extern crate self as editor_sources;

mod sources;
pub use sources::*;
