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
    pub theme: aqua_protocol::shell::ColorScheme,
    pub reduced_motion: bool,
    pub high_contrast: bool,
    pub text_scale: f32,
}

impl Default for SystemAccess {
    fn default() -> Self {
        Self {
            theme: aqua_protocol::shell::ColorScheme::Unknown,
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
            theme: prefs.theme,
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
        let chosen = core.settings().preferences();
        Self {
            theme: match (chosen.theme, system.theme) {
                (editor_core::ThemePreference::Light, _)
                | (
                    editor_core::ThemePreference::System,
                    aqua_protocol::shell::ColorScheme::Light,
                ) => ThemeMode::Light,
                _ => ThemeMode::Dark,
            },
            fonts: FontSet::chosen(chosen.fonts),
            outline_text: chosen.outline_text.on(),
            reduced_motion: chosen.reduced_motion,
            high_contrast: chosen.high_contrast,
            text_scale: chosen.text_percent.map(|percent| {
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "the declared text sizes are small whole percents"
                )]
                let scale = percent as f32 / 100.0;
                scale
            }),
            streaming: match chosen.streaming {
                editor_core::Streaming::Smooth => StreamingMode::Smooth,
                editor_core::Streaming::Swift => StreamingMode::Swift,
            },
            lower_frame_rate: chosen.lower_refresh_rate.on(),
            vsync: chosen.graphics_vsync.on(),
            system,
        }
    }

    #[must_use]
    pub fn motion(&self) -> Motion {
        if self.reduced_motion.unwrap_or(self.system.reduced_motion) {
            Motion::Reduced
        } else {
            Motion::Full
        }
    }

    #[must_use]
    fn contrast(&self) -> Contrast {
        if self.high_contrast.unwrap_or(self.system.high_contrast) {
            Contrast::High
        } else {
            Contrast::Standard
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
