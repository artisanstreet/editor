//! Maps terminal styles onto `GPUI` highlight runs through a themed palette.

use std::ops::Range;

use gpui::{
    FontStyle, FontWeight, HighlightStyle, Hsla, StrikethroughStyle, UnderlineStyle, hsla_to_rgba,
    px, rgb, rgb_to_hsla,
};

use super::style::{TerminalColor, TerminalLine};
use crate::theme::ArtisanTheme;

/// WCAG contrast below which a foreground on the default background falls
/// back to the surface's default text.
const MIN_CONTRAST: f32 = 2.0;

/// The 16 themed base colours terminal indices 0–15 resolve through.
/// Indices 16–255 and truecolor are the program's own data, as sent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalPalette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the
    /// bright variants in the same order.
    pub base: [Hsla; 16],
    /// Dim (SGR 2) text that names no colour of its own.
    pub dim: Hsla,
}

impl TerminalPalette {
    /// The palette of the theme's mode.
    #[must_use]
    pub fn for_theme(theme: &ArtisanTheme) -> Self {
        Self {
            base: theme
                .colors
                .ansi
                .0
                .map(super::super::theme::Oklch::to_paint),
            dim: theme.colors.text_muted.to_paint(),
        }
    }

    /// Resolves one terminal colour: base indices through the palette, the
    /// xterm cube and grey ramp computed, truecolor as-is.
    #[must_use]
    pub fn resolve(&self, color: TerminalColor) -> Hsla {
        match color {
            TerminalColor::Indexed(index @ 0..=15) => self.base[usize::from(index)],
            TerminalColor::Indexed(index @ 16..=231) => {
                let cube = index - 16;
                let level = |step: u8| if step == 0 { 0 } else { 55 + 40 * step };
                rgb_color(level(cube / 36), level(cube / 6 % 6), level(cube % 6))
            }
            TerminalColor::Indexed(index) => {
                let grey = 8 + 10 * (index - 232);
                rgb_color(grey, grey, grey)
            }
            TerminalColor::Rgb(r, g, b) => rgb_color(r, g, b),
        }
    }
}

/// An opaque colour from 8-bit channels.
fn rgb_color(r: u8, g: u8, b: u8) -> Hsla {
    rgb_to_hsla(rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)))
}

/// WCAG relative luminance of an encoded sRGB colour (alpha ignored).
fn relative_luminance(color: Hsla) -> f32 {
    let rgba = hsla_to_rgba(color);
    let linear = |value: f32| {
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.0722f32.mul_add(
        linear(rgba.blue),
        0.7152f32.mul_add(linear(rgba.green), 0.2126 * linear(rgba.red)),
    )
}

/// WCAG contrast ratio between two colours, `1..=21`.
fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `color` unchanged when it reads on `background`; otherwise the surface's
/// default text, so no colour is ever made up.
fn readable(color: Hsla, background: Hsla, foreground: Hsla) -> Hsla {
    if contrast(color, background) >= MIN_CONTRAST {
        color
    } else {
        foreground
    }
}

/// Highlight runs for one line, for `StyledText::with_highlights`.
///
/// `foreground` and `background` are the surface's default text and fill;
/// inverse swaps against them when a colour is unset. A foreground drawn
/// straight on `background` (no explicit background colour) whose WCAG
/// contrast is below 2.0 takes `foreground` instead, so black-on-dark or
/// low-contrast truecolor stays legible. Bold maps to `FontWeight::BOLD`, dim
/// text in the program's colour to that colour at
/// [`crate::theme::Opacity::TerminalDim`], dim text without a colour of its
/// own to the palette's dim colour, and links
/// are underlined.
#[must_use]
pub fn highlights(
    line: &TerminalLine,
    palette: &TerminalPalette,
    foreground: Hsla,
    background: Hsla,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut runs = Vec::with_capacity(line.runs.len());
    for run in &line.runs {
        let style = &run.style;
        let mut fg = style.fg.map(|color| palette.resolve(color));
        let mut bg = style.bg.map(|color| palette.resolve(color));
        if style.inverse {
            (fg, bg) = (
                Some(bg.unwrap_or(background)),
                Some(fg.unwrap_or(foreground)),
            );
        }
        if bg.is_none() {
            fg = fg.map(|color| readable(color, background, foreground));
        }
        if style.dim {
            fg = Some(fg.map_or(palette.dim, |mut color| {
                color.alpha *= crate::theme::Opacity::TerminalDim.alpha();
                color
            }));
        }
        let highlight = HighlightStyle {
            color: fg,
            background_color: bg,
            font_weight: style.bold.then_some(FontWeight::BOLD),
            font_style: style.italic.then_some(FontStyle::Italic),
            underline: (style.underline || style.link.is_some()).then(|| UnderlineStyle {
                thickness: px(1.0),
                color: None,
                wavy: false,
            }),
            strikethrough: style.strikethrough.then(|| StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            }),
            fade_out: None,
        };
        if highlight != HighlightStyle::default() {
            runs.push((run.range.clone(), highlight));
        }
    }
    runs
}
