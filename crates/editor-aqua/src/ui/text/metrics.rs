use aqua_text::{FontFamily, TextStyle};

use super::editor::Mode;
use crate::ui::theme::{ArtisanTheme, Opacity, ProseTypography, RadiusStep, RadiusTokens};

pub struct EditorMetrics {
    pub text_style: TextStyle,
    pub line_height: f32,
    pub visible_lines: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub radius: f32,
    pub border_width: f32,
    pub foreground: aqua_core::Oklch,
    pub caret: aqua_core::Oklch,
    pub placeholder: aqua_core::Oklch,
    pub background: aqua_core::Oklch,
    pub border: aqua_core::Oklch,
    pub focus_border: aqua_core::Oklch,
    pub selection: aqua_core::Oklch,
    pub disabled_opacity: f32,
}

impl EditorMetrics {
    pub fn resolve(theme: &ArtisanTheme, mode: Mode, bare: bool, inset: bool) -> Self {
        let (size, line_height, visible_lines, mut padding_x, mut padding_y, mut radius) =
            match mode {
                Mode::SingleLine => (14.0, 20.0, 1.0, 6.0, 2.0, 6.0),
                Mode::Document => (
                    16.0,
                    24.0,
                    8.0,
                    theme.spacing.steps(3.0),
                    theme.spacing.steps(3.0),
                    RadiusTokens::value(RadiusStep::Xl),
                ),
                Mode::Prompt => (16.0, 24.0, 1.0, 8.0, 4.0, 0.0),
            };
        if inset {
            padding_x = 12.0;
            padding_y = 10.0;
            radius = RadiusTokens::value(RadiusStep::Lg);
        }
        let colors = theme.colors;
        let source_color = colors.text.to_reference_text_color();
        Self {
            text_style: TextStyle {
                families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
                size_px: size,
                line_height_px: line_height,
                weight: ProseTypography::BODY_WEIGHT,
                letter_spacing_em: Some(theme.typography.tracking_em),
                gpui_source_color: Some([
                    source_color.hue_degrees,
                    source_color.saturation,
                    source_color.lightness,
                    source_color.alpha,
                ]),
                ..TextStyle::default()
            },
            line_height,
            visible_lines,
            padding_x,
            padding_y,
            radius,
            border_width: if mode == Mode::Prompt || bare {
                0.0
            } else {
                1.0
            },
            foreground: colors.text.to_paint(),
            caret: colors.text_muted.to_paint(),
            placeholder: if mode == Mode::Prompt {
                colors.text_muted.to_paint()
            } else {
                colors.text_faint.to_paint()
            },
            background: colors.surface.to_paint(),
            border: colors.line_strong.to_paint(),
            focus_border: colors.ring.to_paint(),
            selection: colors.text_selection.to_paint(),
            disabled_opacity: Opacity::Disabled.alpha(),
        }
    }

    pub fn text_appearance(&self) -> aqua_ui::tree::TextAppearance {
        aqua_ui::tree::TextAppearance {
            selection: self.selection,
            caret: self.caret,
            caret_width: 1.0,
            ime_underline: self.foreground,
            ime_underline_width: 1.0,
        }
    }

    pub fn gpui_text_appearance(theme: &ArtisanTheme) -> aqua_ui::tree::GpuiTextAppearance {
        let foreground = theme.colors.text.to_reference_paint();
        aqua_ui::tree::GpuiTextAppearance {
            selection: theme.colors.text_selection.to_reference_paint(),
            caret: theme.colors.text_muted.to_reference_paint(),
            caret_width: 1.0,
            ime_underline: foreground,
            ime_underline_width: 1.0,
        }
    }

    pub fn min_height(&self) -> f32 {
        self.line_height * self.visible_lines + self.padding_y * 2.0 + self.border_width * 2.0
    }

    pub fn prompt_max_height(&self, viewport_height: f32) -> f32 {
        (viewport_height * 0.4).max(self.min_height() * 2.0)
    }
}
