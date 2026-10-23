//! The redraw rate (`native_frame_rate`), applied to the window.
//!
//! The setting is kept in this device's `settings.json`. By default the
//! window draws at its monitor's refresh rate: the platform frame clock waits on the

//! vblank of whichever monitor the window is on, so moving it between a
//! 60 Hz and a 165 Hz monitor retimes it. The setting caps that at 60.

use editor_core::Core;
use gpui::{App, Entity, Window};
use serde_json::Value;
use std::num::NonZeroU32;

/// Whether redraws are capped at [`LOWER_REFRESH_RATE`]; default off.
pub const LOWER_REFRESH_RATE_KEY: &str = "lower_refresh_rate";
const LOWER_REFRESH_RATE: NonZeroU32 = NonZeroU32::new(60).unwrap();

/// Whether the lower refresh rate is on.
pub fn lowered(core: &Core) -> bool {
    core.settings
        .get(LOWER_REFRESH_RATE_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// Applies the stored rate to a newly opened window.
pub fn initialize(core: &Core, window: &mut Window) {
    set_lowered(lowered(core), window);
}

fn set_lowered(lowered: bool, window: &mut Window) {
    window.set_max_frame_rate(lowered.then_some(LOWER_REFRESH_RATE));
    window.set_vsync(true);
}

/// Applies the rate now and stores it.
pub fn apply(core: &Entity<Core>, lowered: bool, window: &mut Window, cx: &mut App) {
    set_lowered(lowered, window);
    core.update(cx, |core, cx| {
        core.set_setting(LOWER_REFRESH_RATE_KEY, Value::Bool(lowered), cx);
    });
}
