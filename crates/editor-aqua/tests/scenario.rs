#![cfg(feature = "qualification")]

#[path = "scenario/forges.rs"]
mod forges;
#[path = "scenario/model_picker.rs"]
mod model_picker;
#[path = "scenario/palette.rs"]
mod palette;
#[path = "scenario/settings.rs"]
mod settings;
#[path = "scenario/sidebar.rs"]
mod sidebar;
#[path = "scenario/states.rs"]
mod states;
#[path = "scenario/thread.rs"]
mod thread;
#[cfg(windows)]
#[path = "scenario/uia.rs"]
mod uia;
