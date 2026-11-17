#![expect(
    clippy::assertions_on_result_states,
    clippy::clone_on_ref_ptr,
    clippy::float_cmp,
    clippy::items_after_statements,
    clippy::let_underscore_untyped,
    clippy::redundant_clone,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::unchecked_time_subtraction,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![cfg(test)]

#[path = "composer/draft_session.rs"]
mod draft_session;
#[path = "composer/encode.rs"]
mod encode;
#[path = "composer/model_line.rs"]
mod model_line;
#[path = "composer/model_list.rs"]
mod model_list;
#[path = "composer/persistence.rs"]
mod persistence;
#[path = "composer/picker.rs"]
mod picker;
#[path = "composer/questionnaire.rs"]
mod questionnaire;
#[path = "composer/sliding_tab.rs"]
mod sliding_tab;
#[cfg(windows)]
#[path = "composer/tsf.rs"]
mod tsf;
#[path = "composer/view.rs"]
mod view;
