//! Styled-text values the terminal model produces: colours, styles, and
//! materialized lines, plus SGR application onto a style.

use std::ops::Range;
use std::sync::Arc;

use super::parser::Params;

/// One SGR colour as the program asked for it, before a palette resolves it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalColor {
    /// An xterm 256-colour index: 0–15 are the themed base colours, 16–231
    /// the 6×6×6 cube, 232–255 the grey ramp.
    Indexed(u8),
    /// A 24-bit truecolor value.
    Rgb(u8, u8, u8),
}

/// The graphic rendition of one span of terminal text.
///
/// `Default` is plain text in the theme's foreground on its background.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent SGR attribute and stays public data"
)]
pub struct TerminalStyle {
    /// Foreground colour; `None` is the theme foreground.
    pub fg: Option<TerminalColor>,
    /// Background colour; `None` is the theme background.
    pub bg: Option<TerminalColor>,
    /// SGR 1.
    pub bold: bool,
    /// SGR 2 (faint).
    pub dim: bool,
    /// SGR 3.
    pub italic: bool,
    /// SGR 4.
    pub underline: bool,
    /// SGR 7: foreground and background swap at paint time.
    pub inverse: bool,
    /// SGR 9.
    pub strikethrough: bool,
    /// OSC 8 hyperlink target. SGR 0 leaves it set, as terminals do; only
    /// another OSC 8 changes it.
    pub link: Option<Arc<str>>,
}

impl TerminalStyle {
    /// Whether this is plain, unlinked default text.
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Applies one SGR (`CSI … m`) parameter list.
    pub(super) fn apply_sgr(&mut self, params: &Params) {
        let mut index = 0;
        while index < params.len() {
            let group = params.group(index);
            let code = group.first().copied().unwrap_or(0);
            index += 1;
            match code {
                38 | 48 | 58 => {
                    let color = if group.len() > 1 {
                        colon_color(&group[1..])
                    } else {
                        let (color, used) = semicolon_color(params, index);
                        index += used;
                        color
                    };
                    // 58 (underline colour) is consumed so its operands are
                    // never misread as attributes, then dropped.
                    match (code, color) {
                        (38, Some(color)) => self.fg = Some(color),
                        (48, Some(color)) => self.bg = Some(color),
                        _ => {}
                    }
                }
                // `4:0` is "no underline"; `4:1`…`4:5` are underline shapes.
                4 if group.len() > 1 => self.underline = group[1] != 0,
                other => self.apply_simple(other),
            }
        }
    }

    /// Applies one single-number SGR code; unknown codes are ignored.
    fn apply_simple(&mut self, code: u32) {
        match code {
            0 => {
                let link = self.link.take();
                *self = Self {
                    link,
                    ..Self::default()
                };
            }
            1 => self.bold = true,
            2 => self.dim = true,
            3 => self.italic = true,
            4 => self.underline = true,
            7 => self.inverse = true,
            9 => self.strikethrough = true,
            // 21 is double underline in xterm but bold-off in older consoles;
            // treating it as a reset never invents emphasis.
            21 => self.bold = false,
            22 => {
                self.bold = false;
                self.dim = false;
            }
            23 => self.italic = false,
            24 => self.underline = false,
            27 => self.inverse = false,
            29 => self.strikethrough = false,
            30..=37 => self.fg = Some(TerminalColor::Indexed(base_index(code - 30))),
            39 => self.fg = None,
            40..=47 => self.bg = Some(TerminalColor::Indexed(base_index(code - 40))),
            49 => self.bg = None,
            90..=97 => self.fg = Some(TerminalColor::Indexed(base_index(code - 90 + 8))),
            100..=107 => self.bg = Some(TerminalColor::Indexed(base_index(code - 100 + 8))),
            _ => {}
        }
    }
}

/// Narrows an already range-checked base-colour offset.
fn base_index(offset: u32) -> u8 {
    u8::try_from(offset).unwrap_or(0)
}

/// Clamps an SGR operand into a colour channel or palette index.
fn channel(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

/// Extended colour from colon sub-parameters after the `38`/`48`:
/// `5:n`, `2:r:g:b`, or `2:space:r:g:b` (ITU T.416 with a colour-space id).
fn colon_color(sub: &[u32]) -> Option<TerminalColor> {
    match sub {
        [5, index, ..] => Some(TerminalColor::Indexed(channel(*index))),
        [2, _, r, g, b, ..] | [2, r, g, b] => {
            Some(TerminalColor::Rgb(channel(*r), channel(*g), channel(*b)))
        }
        _ => None,
    }
}

/// Extended colour from the semicolon form `38;5;n` / `38;2;r;g;b`, reading
/// groups from `start`. Returns the colour and how many groups it consumed.
fn semicolon_color(params: &Params, start: usize) -> (Option<TerminalColor>, usize) {
    let at = |offset: usize| params.group(start + offset).first().copied();
    match at(0) {
        Some(5) => match at(1) {
            Some(index) => (Some(TerminalColor::Indexed(channel(index))), 2),
            None => (None, 1),
        },
        Some(2) => match (at(1), at(2), at(3)) {
            (Some(r), Some(g), Some(b)) => (
                Some(TerminalColor::Rgb(channel(r), channel(g), channel(b))),
                4,
            ),
            _ => (None, params.len().saturating_sub(start)),
        },
        Some(_) => (None, 1),
        None => (None, 0),
    }
}

/// One styled span of a [`TerminalLine`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalRun {
    /// Byte range into [`TerminalLine::text`], always on `char` boundaries.
    pub range: Range<usize>,
    /// The span's style; never [`TerminalStyle::is_default`].
    pub style: TerminalStyle,
}

/// One materialized output line.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalLine {
    /// The visible text, escape sequences removed.
    pub text: String,
    /// Non-default styled spans in order, adjacent equal styles merged.
    /// Default-styled text has no run: anything not covered paints plain.
    pub runs: Vec<TerminalRun>,
}
