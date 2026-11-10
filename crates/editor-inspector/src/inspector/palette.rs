use editor_ui::{
    markdown::terminal::{TerminalColor, TerminalLine},
    theme::{ArtisanTheme, Opacity},
};
use aqua_text::{FontSlant, FontWeight, StyledSpan, TextStyle};
use palette::{Hsla, IntoColor, Srgba};

#[derive(Clone, Copy)]
pub struct TerminalPalette {
    pub base: [[f32; 4]; 16],
    pub dim: [f32; 4],
}

fn theme_color(color: editor_ui::theme::Oklch) -> [f32; 4] {
    let value = color.to_reference_text_color();
    [
        value.hue_degrees,
        value.saturation,
        value.lightness,
        value.alpha,
    ]
}

fn rgb(r: u8, g: u8, b: u8) -> [f32; 4] {
    let color: Hsla = Srgba::new(
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        1.0,
    )
    .into_color();
    [
        color.hue.into_raw_degrees(),
        color.saturation,
        color.lightness,
        color.alpha,
    ]
}

fn luminance(color: [f32; 4]) -> f32 {
    let color = Hsla::new(color[0], color[1], color[2], color[3]);
    let rgb: Srgba = color.into_color();
    let linear = |value: f32| {
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.0722_f32.mul_add(
        linear(rgb.blue),
        0.7152_f32.mul_add(linear(rgb.green), 0.2126 * linear(rgb.red)),
    )
}

impl TerminalPalette {
    pub fn for_theme(theme: &ArtisanTheme) -> Self {
        Self {
            base: theme.colors.ansi.0.map(theme_color),
            dim: theme_color(theme.colors.text_muted),
        }
    }

    pub fn resolve(&self, color: TerminalColor) -> [f32; 4] {
        match color {
            TerminalColor::Indexed(index @ 0..=15) => self.base[usize::from(index)],
            TerminalColor::Indexed(index @ 16..=231) => {
                let cube = index - 16;
                let level = |step| if step == 0 { 0 } else { 55 + 40 * step };
                rgb(level(cube / 36), level(cube / 6 % 6), level(cube % 6))
            }
            TerminalColor::Indexed(index) => {
                let grey = 8 + 10 * (index - 232);
                rgb(grey, grey, grey)
            }
            TerminalColor::Rgb(r, g, b) => rgb(r, g, b),
        }
    }

    pub fn spans(
        &self,
        line: &TerminalLine,
        base: &TextStyle,
        foreground: [f32; 4],
        background: [f32; 4],
    ) -> Vec<StyledSpan> {
        let mut spans = Vec::new();
        let mut cursor = 0;
        for run in &line.runs {
            if run.range.start > cursor {
                spans.push(StyledSpan::new(
                    &line.text[cursor..run.range.start],
                    base.clone(),
                ));
            }
            let style = &run.style;
            let mut fg = style.fg.map(|color| self.resolve(color));
            let mut bg = style.bg.map(|color| self.resolve(color));
            if style.inverse {
                (fg, bg) = (
                    Some(bg.unwrap_or(background)),
                    Some(fg.unwrap_or(foreground)),
                );
            }
            if bg.is_none() {
                fg = fg.map(|color| {
                    let (a, b) = (luminance(color), luminance(background));
                    if (a.max(b) + 0.05) / (a.min(b) + 0.05) >= 2.0 {
                        color
                    } else {
                        foreground
                    }
                });
            }
            if style.dim {
                fg = Some(fg.map_or(self.dim, |mut color| {
                    color[3] *= Opacity::TerminalDim.alpha();
                    color
                }));
            }
            let mut font = base.clone();
            if style.bold {
                font.weight = FontWeight::BOLD;
            }
            if style.italic {
                font.slant = FontSlant::Italic;
            }
            font.underline = style.underline || style.link.is_some();
            font.strikethrough = style.strikethrough;
            let mut span = StyledSpan::new(&line.text[run.range.clone()], font);
            if let Some(color) = fg {
                span = span.gpui_source_color(color);
            }
            if let Some(color) = bg {
                span = span.gpui_background([
                    color[0].rem_euclid(360.0) / 360.0,
                    color[1],
                    color[2],
                    color[3],
                ]);
            }
            spans.push(span);
            cursor = run.range.end;
        }
        if cursor < line.text.len() {
            spans.push(StyledSpan::new(&line.text[cursor..], base.clone()));
        }
        spans
    }
}
