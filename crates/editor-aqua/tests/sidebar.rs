#![expect(
    clippy::float_cmp,
    clippy::inconsistent_struct_constructor,
    clippy::manual_midpoint,
    clippy::redundant_clone,
    clippy::string_slice,
    clippy::too_many_lines,
    clippy::wildcard_enum_match_arm,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "sidebar/mod.rs"]
mod sidebar;

#[path = "sidebar/drag_preview.rs"]
mod drag_preview;

#[path = "sidebar/settings_nav.rs"]
mod settings_nav;

#[path = "sidebar/geometry.rs"]
mod geometry;
