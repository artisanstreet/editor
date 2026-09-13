//! Local redraw preference, independent of Forge and model configuration.

use gpui::{App, Global, Window};
use std::{
    num::NonZeroU32,
    path::{Path, PathBuf},
};

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

    fn parse(value: &str) -> Option<Self> {
        Self::OPTIONS
            .iter()
            .copied()
            .find(|limit| limit.label() == value.trim())
    }
}

struct FrameRatePreference {
    limit: FrameRateLimit,
    path: Option<PathBuf>,
}
impl Global for FrameRatePreference {}

pub(crate) fn initialize(window: &mut Window, cx: &mut App) {
    let path = artisan_editor_cli::paths::Layout::discover()
        .ok()
        .map(|layout| layout.root.join("ui").join("frame-rate-limit"));
    let limit = path
        .as_ref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|value| FrameRateLimit::parse(&value))
        .unwrap_or_default();
    window.set_max_frame_rate(limit.0);
    window.set_vsync(limit.0.is_some());
    cx.set_global(FrameRatePreference { limit, path });
}

pub(crate) fn current(cx: &App) -> FrameRateLimit {
    cx.try_global::<FrameRatePreference>()
        .map_or(FrameRateLimit::default(), |preference| preference.limit)
}

pub(crate) fn apply(
    limit: FrameRateLimit,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    window.set_max_frame_rate(limit.0);
    window.set_vsync(limit.0.is_some());
    let preference = cx.try_global::<FrameRatePreference>();
    let path = preference.and_then(|preference| preference.path.clone());
    cx.set_global(FrameRatePreference {
        limit,
        path: path.clone(),
    });
    let path = path.ok_or_else(|| {
        "Applied for this session. The settings location is unavailable.".to_owned()
    })?;
    save(&path, limit)
        .map_err(|_| "Applied for this session, but the FPS limit could not be saved.".to_owned())
}

fn save(path: &Path, limit: FrameRateLimit) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let pending = path.with_extension("pending");
    std::fs::write(&pending, limit.label())?;
    std::fs::rename(pending, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn settings_select_applies_and_saves_the_limit(cx: &mut gpui::TestAppContext) {
        let path = std::env::temp_dir()
            .join(format!("artisan-frame-rate-ui-test-{}", std::process::id()))
            .join("limit");
        cx.update(|app| {
            app.set_global(FrameRatePreference {
                limit: FrameRateLimit::default(),
                path: Some(path.clone()),
            });
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
            assert_eq!(std::fs::read_to_string(&path).unwrap(), value);
        }
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
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

    #[test]
    fn saving_replaces_the_previous_preference() {
        let path = std::env::temp_dir()
            .join(format!("artisan-frame-rate-test-{}", std::process::id()))
            .join("limit");
        save(&path, FrameRateLimit::parse("60").unwrap()).unwrap();
        save(&path, FrameRateLimit::parse("240").unwrap()).unwrap();
        assert_eq!(
            FrameRateLimit::parse(&std::fs::read_to_string(&path).unwrap()),
            FrameRateLimit::parse("240")
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
