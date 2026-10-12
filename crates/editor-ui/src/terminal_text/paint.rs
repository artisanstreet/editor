//! Maps terminal styles onto `GPUI` highlight runs through a themed palette.

use std::ops::Range;

use gpui::{
    FontStyle, FontWeight, HighlightStyle, Hsla, StrikethroughStyle, UnderlineStyle, hsla_to_rgba,
    px, rgb, rgb_to_hsla,
};

use super::style::{TerminalColor, TerminalLine};
use crate::theme::{ArtisanTheme, ThemeMode};

/// Alpha applied to dim (SGR 2) text.
const DIM_ALPHA: f32 = 0.6;
/// WCAG contrast below which a foreground on the default background is
/// nudged toward readable.
const MIN_CONTRAST: f32 = 2.0;
/// Lightness step of that nudge.
const NUDGE_STEP: f32 = 0.05;

/// Dark-mode base colours, after the VS Code dark terminal: saturated enough
/// to read as the program meant on the `S900` popover surface.
const DARK_BASE: [u32; 16] = [
    0x0000_0000,
    0x00cd_3131,
    0x000d_bc79,
    0x00e5_e510,
    0x0024_72c8,
    0x00bc_3fbc,
    0x0011_a8cd,
    0x00e5_e5e5,
    0x0066_6666,
    0x00f1_4c4c,
    0x0023_d18b,
    0x00f5_f543,
    0x003b_8eea,
    0x00d6_70d6,
    0x0029_b8db,
    0x00e5_e5e5,
];

/// Light-mode base colours, after the VS Code light terminal with the bright
/// green and yellow darkened so they hold up on the white popover surface.
const LIGHT_BASE: [u32; 16] = [
    0x0000_0000,
    0x00cd_3131,
    0x0010_7c10,
    0x0094_9800,
    0x0004_51a5,
    0x00bc_05bc,
    0x0005_98bc,
    0x0055_5555,
    0x0066_6666,
    0x00cd_3131,
    0x0014_a814,
    0x00a6_a800,
    0x0004_51a5,
    0x00bc_05bc,
    0x0005_98bc,
    0x00a5_a5a5,
];

/// The 16 themed base colours terminal indices 0–15 resolve through.
/// Indices 16–255 and truecolor are fixed and need no palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalPalette {
    /// Black, red, green, yellow, blue, magenta, cyan, white, then the
    /// bright variants in the same order.
    pub base: [Hsla; 16],
}

impl TerminalPalette {
    /// The palette for one theme mode.
    #[must_use]
    pub fn for_mode(mode: ThemeMode) -> Self {
        let hexes = match mode {
            ThemeMode::Light => LIGHT_BASE,
            ThemeMode::Dark => DARK_BASE,
        };
        Self {
            base: hexes.map(|hex| rgb_to_hsla(rgb(hex))),
        }
    }

    /// The palette for `theme`'s mode.
    #[must_use]
    pub fn for_theme(theme: &ArtisanTheme) -> Self {
        Self::for_mode(theme.mode)
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
    0.2126 * linear(rgba.red) + 0.7152 * linear(rgba.green) + 0.0722 * linear(rgba.blue)
}

/// WCAG contrast ratio between two colours, `1..=21`.
fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (relative_luminance(a), relative_luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// `color` unchanged when it reads on `background`; otherwise its lightness
/// is stepped away from the background until it does (or hits the end).
fn readable(color: Hsla, background: Hsla) -> Hsla {
    if contrast(color, background) >= MIN_CONTRAST {
        return color;
    }
    let step = if relative_luminance(background) < 0.18 {
        NUDGE_STEP
    } else {
        -NUDGE_STEP
    };
    let mut adjusted = color;
    while (0.0..=1.0).contains(&(adjusted.lightness + step)) {
        adjusted.lightness += step;
        if contrast(adjusted, background) >= MIN_CONTRAST {
            break;
        }
    }
    adjusted
}

/// Highlight runs for one line, for `StyledText::with_highlights`.
///
/// `foreground` and `background` are the surface's default text and fill;
/// inverse swaps against them when a colour is unset. A foreground drawn
/// straight on `background` (no explicit background colour) whose WCAG
/// contrast is below 2.0 is nudged toward readable, so black-on-dark or
/// low-contrast truecolor stays legible. Bold maps to `FontWeight::BOLD`, dim
/// to 60% alpha, and links are underlined.
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
            fg = fg.map(|color| readable(color, background));
        }
        if style.dim {
            let mut color = fg.unwrap_or(foreground);
            color.alpha *= DIM_ALPHA;
            fg = Some(color);
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
