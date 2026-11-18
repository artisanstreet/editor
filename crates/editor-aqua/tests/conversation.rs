#![expect(
    clippy::assertions_on_result_states,
    clippy::cast_possible_truncation,
    clippy::clone_on_ref_ptr,
    clippy::float_cmp,
    clippy::iter_on_single_items,
    clippy::let_underscore_untyped,
    clippy::needless_borrow,
    clippy::redundant_clone,
    clippy::renamed_function_params,
    clippy::semicolon_if_nothing_returned,
    clippy::suboptimal_flops,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    let_underscore_drop,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![cfg(test)]

#[path = "conversation/compaction_view.rs"]
mod compaction_view;
#[path = "conversation/host_lifetime.rs"]
mod host_lifetime;
#[path = "conversation/motion.rs"]
mod motion;
#[path = "conversation/parked.rs"]
mod parked;
#[path = "conversation/presentation.rs"]
mod presentation;
#[path = "conversation/projection.rs"]
mod projection;
#[path = "conversation/questions.rs"]
mod questions;
#[path = "conversation/read.rs"]
mod read;
#[path = "conversation/request_rows.rs"]
mod request_rows;
#[path = "conversation/retention.rs"]
mod retention;
#[path = "conversation/source_owners.rs"]
mod source_owners;
#[path = "conversation/stored_paging.rs"]
mod stored_paging;
#[path = "conversation/stored_rows.rs"]
mod stored_rows;
#[path = "conversation/thread.rs"]
mod thread;
#[path = "conversation/tool_rows.rs"]
mod tool_rows;
#[path = "conversation/viewport.rs"]
mod viewport;

#[path = "conversation/row_cache.rs"]
mod row_cache;
#[path = "conversation/view/transcript.rs"]
mod transcript;
#[path = "conversation/view/drawer.rs"]
mod view_drawer;
