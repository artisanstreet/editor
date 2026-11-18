#![expect(
    clippy::clone_on_ref_ptr,
    clippy::let_underscore_must_use,
    clippy::let_underscore_untyped,
    clippy::redundant_clone,
    clippy::string_slice,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "media/color.rs"]
mod color;
#[path = "media/source.rs"]
mod source;

#[path = "ui/media/audio.rs"]
mod audio;
