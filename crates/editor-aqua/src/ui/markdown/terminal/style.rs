use std::ops::Range;
use std::sync::Arc;

use super::parser::Params;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalColor {
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent SGR attribute and stays public data"
)]
pub struct TerminalStyle {
    pub fg: Option<TerminalColor>,
    pub bg: Option<TerminalColor>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    pub strikethrough: bool,
    pub link: Option<Arc<str>>,
}

impl TerminalStyle {
    #[must_use]
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

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
                    match (code, color) {
                        (38, Some(color)) => self.fg = Some(color),
                        (48, Some(color)) => self.bg = Some(color),
                        _ => {}
                    }
                }
                4 if group.len() > 1 => self.underline = group[1] != 0,
                other => self.apply_simple(other),
            }
        }
    }

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

fn base_index(offset: u32) -> u8 {
    u8::try_from(offset).unwrap_or(0)
}

fn channel(value: u32) -> u8 {
    u8::try_from(value).unwrap_or(u8::MAX)
}

fn colon_color(sub: &[u32]) -> Option<TerminalColor> {
    match sub {
        [5, index, ..] => Some(TerminalColor::Indexed(channel(*index))),
        [2, _, r, g, b, ..] | [2, r, g, b] => {
            Some(TerminalColor::Rgb(channel(*r), channel(*g), channel(*b)))
        }
        _ => None,
    }
}

fn semicolon_color(params: &Params, start: usize) -> (Option<TerminalColor>, usize) {
    let at = |offset: usize| params.group(start + offset).first().copied();
    match at(0) {
        Some(5) => at(1).map_or((None, 1), |index| {
            (Some(TerminalColor::Indexed(channel(index))), 2)
        }),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalRun {
    pub range: Range<usize>,
    pub style: TerminalStyle,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TerminalLine {
    pub text: String,
    pub runs: Vec<TerminalRun>,
}
