//! The redraw limit and the FPS overlay (`native_frame_rate`), kept in this
//! device's `settings.json` and applied to the window.

use editor_core::Core;
use gpui::{App, DebugFrameOverlayMode, Entity, Window};
use serde_json::Value;
use std::num::NonZeroU32;

/// Frames per second, or `null` for unlimited.
pub const FRAME_RATE_LIMIT_KEY: &str = "frame_rate_limit";
/// Whether the FPS overlay shows; default on, as before.
pub const FPS_OVERLAY_KEY: &str = "fps_overlay";

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct FrameRateLimit(Option<NonZeroU32>);

impl FrameRateLimit {
    pub const OPTIONS: [Self; 10] = [
        Self(NonZeroU32::new(30)),
        Self(NonZeroU32::new(60)),
        Self(NonZeroU32::new(120)),
        Self(NonZeroU32::new(144)),
        Self(NonZeroU32::new(165)),
        Self(NonZeroU32::new(180)),
        Self(NonZeroU32::new(240)),
        Self(NonZeroU32::new(360)),
        Self(NonZeroU32::new(500)),
        Self(None),
    ];

    #[must_use]
    pub fn label(self) -> String {
        self.0
            .map_or_else(|| "Unlimited".into(), |fps| fps.to_string())
    }

    fn value(self) -> Value {
        self.0.map_or(Value::Null, |fps| Value::from(fps.get()))
    }

    fn from_value(value: Option<&Value>) -> Self {
        let fps = value
            .and_then(Value::as_u64)
            .and_then(|fps| u32::try_from(fps).ok())
            .and_then(NonZeroU32::new);
        Self::OPTIONS
            .into_iter()
            .find(|limit| limit.0 == fps)
            .unwrap_or_default()
    }
}

/// The stored limit.
pub fn current(core: &Core) -> FrameRateLimit {
    FrameRateLimit::from_value(core.settings.get(FRAME_RATE_LIMIT_KEY))
}

/// Whether the overlay is on.
pub fn overlay_visible(core: &Core) -> bool {
    core.settings
        .get(FPS_OVERLAY_KEY)
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

/// Applies the stored limit and overlay to a newly opened window.
pub fn initialize(core: &Core, window: &mut Window) {
    set_limit(current(core), window);
    set_overlay(overlay_visible(core), window);
}

fn set_limit(limit: FrameRateLimit, window: &mut Window) {
    window.set_max_frame_rate(limit.0);
    window.set_vsync(limit.0.is_some());
}

fn set_overlay(visible: bool, window: &mut Window) {
    window.set_debug_frame_overlay_mode(if visible {
        DebugFrameOverlayMode::FrameRate
    } else {
        DebugFrameOverlayMode::Hidden
    });
}

/// Applies `limit` now and stores it.
pub fn apply(core: &Entity<Core>, limit: FrameRateLimit, window: &mut Window, cx: &mut App) {
    set_limit(limit, window);
    core.update(cx, |core, cx| {
        core.set_setting(FRAME_RATE_LIMIT_KEY, limit.value(), cx);
    });
}

/// Shows or hides the overlay now and stores it.
pub fn apply_overlay(core: &Entity<Core>, visible: bool, window: &mut Window, cx: &mut App) {
    set_overlay(visible, window);
    core.update(cx, |core, cx| {
        core.set_setting(FPS_OVERLAY_KEY, Value::Bool(visible), cx);
    });
}

#[cfg(test)]
mod tests {
    use super::FrameRateLimit;
    use serde_json::json;

    #[test]
    fn the_limit_round_trips_through_settings_json() {
        for limit in FrameRateLimit::OPTIONS {
            assert_eq!(FrameRateLimit::from_value(Some(&limit.value())), limit);
        }
        assert_eq!(
            FrameRateLimit::from_value(Some(&json!(77))).label(),
            "Unlimited"
        );
        assert_eq!(FrameRateLimit::OPTIONS[1].label(), "60");
    }
}
