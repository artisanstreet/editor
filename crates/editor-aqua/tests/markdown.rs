#![expect(
    clippy::clone_on_ref_ptr,
    clippy::float_cmp,
    clippy::string_slice,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "markdown/presenter.rs"]
mod presenter;
#[path = "markdown/reveal.rs"]
mod reveal;
#[cfg(feature = "qualification")]
#[expect(
    clippy::default_trait_access,
    clippy::format_collect,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::missing_assert_message,
    clippy::needless_borrow,
    clippy::needless_pass_by_value,
    clippy::redundant_clone,
    clippy::unwrap_used,
    let_underscore_drop,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "markdown/transport.rs"]
mod transport;
#[expect(
    clippy::disallowed_types,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "markdown/visual.rs"]
mod visual;
