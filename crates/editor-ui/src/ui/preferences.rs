use crate::ui::theme::{ArtisanTheme, Contrast, FontSet, Motion, ThemeMode, TypographyTokens};
use aqua::AppCtx;
use aqua::prelude::{AquaWindowId, WindowRequest};
use aqua_core::scene::TextRasterMode;
use editor_core::Core;
use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowSetting {
    TextRaster,
    FrameRate,
    Vsync,
}

#[derive(Debug, thiserror::Error)]
#[error("apply the {setting:?} preference to the window")]
pub struct ApplyWindowError {
    setting: WindowSetting,
    #[source]
    source: aqua_protocol::window::WindowError,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamingMode {
    #[default]
    Smooth,
    Swift,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemAccess {
    pub reduced_motion: bool,
    pub high_contrast: bool,
    pub text_scale: f32,
}

impl Default for SystemAccess {
    fn default() -> Self {
        Self {
            reduced_motion: false,
            high_contrast: false,
            text_scale: 1.0,
        }
    }
}

impl SystemAccess {
    #[must_use]
    pub fn from_os(prefs: &aqua::prelude::OsPreferences) -> Self {
        Self {
            reduced_motion: prefs.reduce_motion.is_on(),
            high_contrast: prefs.high_contrast.is_on(),
            text_scale: prefs.text_scale.factor(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Preferences {
    pub theme: ThemeMode,
    pub fonts: FontSet,
    pub outline_text: bool,
    pub reduced_motion: Option<bool>,
    pub high_contrast: Option<bool>,
    pub text_scale: Option<f32>,
    pub streaming: StreamingMode,
    pub lower_frame_rate: bool,
    pub vsync: bool,
    pub system: SystemAccess,
}

impl Preferences {
    pub fn from_core(core: &Core, system: SystemAccess) -> Self {
        let settings = &core.settings;
        let boolean = |key: &str| settings.get(key).and_then(serde_json::Value::as_bool);
        let string = |key: &str| settings.get(key).and_then(serde_json::Value::as_str);
        Self {
            theme: if boolean("dark_mode").unwrap_or(true) {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            fonts: FontSet::from_key(string("font_set")),
            outline_text: boolean("outline_text").unwrap_or(true),
            reduced_motion: boolean("reduced_motion"),
            high_contrast: boolean("high_contrast"),
            text_scale: settings
                .get("text_scale")
                .and_then(serde_json::Value::as_f64)
                .filter(|percent| percent.is_finite())
                .map(|percent| (percent.clamp(100.0, 225.0) / 100.0) as f32),
            streaming: if string("streaming_mode") == Some("swift") {
                StreamingMode::Swift
            } else {
                StreamingMode::Smooth
            },
            lower_frame_rate: boolean("lower_refresh_rate").unwrap_or(false),
            vsync: boolean("graphics_vsync").unwrap_or(true),
            system,
        }
    }

    #[must_use]
    pub fn motion(&self) -> Motion {
        match self.reduced_motion.unwrap_or(self.system.reduced_motion) {
            true => Motion::Reduced,
            false => Motion::Full,
        }
    }

    #[must_use]
    fn contrast(&self) -> Contrast {
        match self.high_contrast.unwrap_or(self.system.high_contrast) {
            true => Contrast::High,
            false => Contrast::Standard,
        }
    }

    pub fn theme(&self) -> ArtisanTheme {
        let mut theme = *ArtisanTheme::for_mode_and_contrast(self.theme, self.contrast());
        theme.typography = TypographyTokens::new(self.fonts)
            .scaled(self.text_scale.unwrap_or(self.system.text_scale));
        theme.interaction.motion = self.motion();
        theme
    }

    pub fn apply_window(
        &self,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> Result<(), ApplyWindowError> {
        let failed = |setting| move |source| ApplyWindowError { setting, source };
        ctx.request_window(WindowRequest::SetTextRasterMode(
            window,
            if self.outline_text {
                TextRasterMode::Outline
            } else {
                TextRasterMode::PlatformDefault
            },
        ))
        .map_err(failed(WindowSetting::TextRaster))?;
        ctx.request_window(WindowRequest::SetMaxFrameRate(
            window,
            if self.lower_frame_rate {
                NonZeroU32::new(60)
            } else {
                None
            },
        ))
        .map_err(failed(WindowSetting::FrameRate))?;
        ctx.request_window(WindowRequest::SetVsync(window, self.vsync))
            .map_err(failed(WindowSetting::Vsync))
    }
}
