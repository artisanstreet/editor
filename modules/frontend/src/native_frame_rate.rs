//! Local redraw preference, independent of Forge and model configuration.
//!
//! The limit and overlay live in the Editor pool ([`crate::editor_settings`]).

use crate::editor_settings::{self, SettingsPersistError};
use gpui::{App, Window};
use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(crate) struct FrameRateLimit(Option<NonZeroU32>);

impl FrameRateLimit {
    pub(crate) const OPTIONS: [Self; 10] = [
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

    pub(crate) fn label(self) -> String {
        self.0
            .map_or_else(|| "Unlimited".into(), |fps| fps.to_string())
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::OPTIONS
            .iter()
            .copied()
            .find(|limit| limit.label() == value.trim())
    }
}

/// Applies the stored limit and overlay to a newly opened window.
pub(crate) fn initialize(window: &mut Window, cx: &mut App) {
    let settings = editor_settings::get(cx);
    set_limit(settings.frame_rate_limit(), window);
    set_overlay_mode(settings.fps_overlay(), window);
}

pub(crate) fn current(cx: &App) -> FrameRateLimit {
    editor_settings::get(cx).frame_rate_limit()
}

fn set_limit(limit: FrameRateLimit, window: &mut Window) {
    window.set_max_frame_rate(limit.0);
    window.set_vsync(limit.0.is_some());
}

/// Maps a persistence failure to the settings screen's session-only notice.
fn saved(result: Result<(), SettingsPersistError>, setting: &str) -> Result<(), String> {
    result.map_err(|error| match error {
        SettingsPersistError::Unavailable => {
            "Applied for this session. The settings location is unavailable.".to_owned()
        }
        SettingsPersistError::Io(_) => {
            format!("Applied for this session, but the {setting} could not be saved.")
        }
    })
}

pub(crate) fn apply(
    limit: FrameRateLimit,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    set_limit(limit, window);
    saved(
        editor_settings::update(cx, |settings| settings.with_frame_rate_limit(limit)),
        "FPS limit",
    )
}

pub(crate) fn overlay_visible(cx: &App) -> bool {
    editor_settings::get(cx).fps_overlay()
}

fn set_overlay_mode(visible: bool, window: &mut Window) {
    window.set_debug_frame_overlay_mode(if visible {
        gpui::DebugFrameOverlayMode::FrameRate
    } else {
        gpui::DebugFrameOverlayMode::Hidden
    });
}

pub(crate) fn apply_overlay(
    visible: bool,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    set_overlay_mode(visible, window);
    saved(
        editor_settings::update(cx, |settings| settings.with_fps_overlay(visible)),
        "FPS overlay setting",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored(path: &std::path::Path, key: &str) -> serde_json::Value {
        let bytes = std::fs::read(path).expect("saved settings");
        serde_json::from_slice::<serde_json::Value>(&bytes).expect("settings json")[key].clone()
    }

    #[gpui::test]
    fn settings_select_applies_and_saves_the_limit(cx: &mut gpui::TestAppContext) {
        let root =
            std::env::temp_dir().join(format!("artisan-frame-rate-ui-test-{}", std::process::id()));
        let path = editor_settings::settings_path(&root);
        cx.update(|app| {
            editor_settings::install(
                editor_settings::EditorSettings::default(),
                Some(path.clone()),
                app,
            );
        });
        let (_, cx) = cx.add_window_view(|_, cx| {
            crate::native_settings::SettingsScreen::new(
                crate::native_route::SettingsRoute::Appearance,
                None,
                artisan_ui::theme::ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(gpui::px(1100.0), gpui::px(1800.0)));
        cx.run_until_parked();
        for value in ["120", "Unlimited"] {
            let limit = FrameRateLimit::parse(value).unwrap();
            let section_before = cx.debug_bounds("settings-section-performance").unwrap();
            let trigger = cx
                .debug_bounds("settings-frame-rate-limit-trigger")
                .expect("FPS select trigger");
            cx.simulate_click(trigger.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            assert!(
                cx.debug_bounds("settings-frame-rate-limit-content")
                    .is_some()
            );
            let section_open = cx.debug_bounds("settings-section-performance").unwrap();
            assert_eq!(
                section_before, section_open,
                "opening options must not resize the card"
            );
            let content = cx
                .debug_bounds("settings-frame-rate-limit-content")
                .unwrap();
            assert!(content.top() >= gpui::px(0.0));
            assert!(content.bottom() <= gpui::px(1800.0));
            assert!(trigger.size.width <= gpui::px(144.0));
            if value == "120" {
                cx.simulate_keystrokes("home down down enter");
            } else {
                cx.simulate_keystrokes("end enter");
            }
            cx.run_until_parked();
            cx.update(|window, app| {
                assert_eq!(window.max_frame_rate(), limit.0);
                assert_eq!(current(app), limit);
            });
            assert_eq!(stored(&path, "frame_rate_limit"), value);
        }
        for visible in [false, true] {
            let toggle = cx
                .debug_bounds("settings-fps-overlay")
                .expect("FPS overlay toggle");
            cx.simulate_click(toggle.center(), gpui::Modifiers::default());
            cx.run_until_parked();
            cx.update(|window, app| {
                assert_eq!(overlay_visible(app), visible);
                assert_eq!(
                    window.debug_frame_overlay_mode() == gpui::DebugFrameOverlayMode::FrameRate,
                    visible
                );
                assert_eq!(current(app), FrameRateLimit::default());
            });
            assert_eq!(stored(&path, "fps_overlay"), visible);
            assert_eq!(stored(&path, "frame_rate_limit"), "Unlimited");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[gpui::test]
    fn changes_without_a_settings_location_apply_for_the_session(cx: &mut gpui::TestAppContext) {
        let cx = cx.add_empty_window();
        cx.update(|window, app| {
            let limit = FrameRateLimit::parse("60").unwrap();
            assert_eq!(
                apply(limit, window, app),
                Err("Applied for this session. The settings location is unavailable.".to_owned())
            );
            assert_eq!(current(app), limit);
            assert_eq!(window.max_frame_rate(), limit.0);
            assert!(apply_overlay(false, window, app).is_err());
            assert!(!overlay_visible(app));
        });
    }

    #[test]
    fn supported_values_round_trip_and_invalid_values_are_rejected() {
        let labels = FrameRateLimit::OPTIONS
            .iter()
            .map(|limit| limit.label())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            [
                "30",
                "60",
                "120",
                "144",
                "165",
                "180",
                "240",
                "360",
                "500",
                "Unlimited"
            ]
        );
        for limit in FrameRateLimit::OPTIONS {
            assert_eq!(FrameRateLimit::parse(&limit.label()), Some(limit));
        }
        for invalid in ["0", "-30", "200", "NaN", ""] {
            assert_eq!(FrameRateLimit::parse(invalid), None);
        }
        assert_eq!(FrameRateLimit::default().label(), "Unlimited");
    }
}
