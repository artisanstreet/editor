use crate::core::Core;
use crate::ui::theme::{ArtisanTheme, FontSet, ThemeMode, TypographyTokens};
use aqua::AppCtx;
use aqua::prelude::{AquaWindowId, WindowRequest};
use aqua_core::scene::TextRasterMode;
use std::num::NonZeroU32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StreamingMode {
    #[default]
    Smooth,
    Swift,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preferences {
    pub theme: ThemeMode,
    pub fonts: FontSet,
    pub outline_text: bool,
    pub reduced_motion: Option<bool>,
    pub streaming: StreamingMode,
    pub lower_frame_rate: bool,
}

impl Preferences {
    pub fn from_core(core: &Core) -> Self {
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
            streaming: if string("streaming_mode") == Some("swift") {
                StreamingMode::Swift
            } else {
                StreamingMode::Smooth
            },
            lower_frame_rate: boolean("lower_refresh_rate").unwrap_or(false),
        }
    }

    pub fn theme(&self) -> ArtisanTheme {
        let mut theme = *ArtisanTheme::for_mode(self.theme);
        theme.typography = TypographyTokens::new(self.fonts);
        theme
    }

    pub fn apply_window(&self, ctx: &mut AppCtx<'_>, window: AquaWindowId) -> anyhow::Result<()> {
        ctx.request_window(WindowRequest::SetTextRasterMode(
            window,
            if self.outline_text {
                TextRasterMode::Outline
            } else {
                TextRasterMode::PlatformDefault
            },
        ))?;
        ctx.request_window(WindowRequest::SetMaxFrameRate(
            window,
            if self.lower_frame_rate {
                NonZeroU32::new(60)
            } else {
                None
            },
        ))?;
        ctx.request_window(WindowRequest::SetVsync(window, true))?;
        Ok(())
    }
}
