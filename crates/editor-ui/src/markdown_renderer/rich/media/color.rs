//! CSS colors: parsing every common notation, converting between spaces,
//! formatting in the notation the reader picks, and checking gamuts.
//!
//! A color is held as unclamped gamma-encoded sRGB plus alpha, so a wide
//! gamut color (Display P3, a vivid OKLCH) survives a round trip and its
//! distance outside sRGB can be reported. Conversions follow CSS Color 4:
//! sRGB and Display P3 through linear light and CIE XYZ (D65), Lab and LCH
//! through XYZ D50 with the Bradford adaptation, and `OKLab` through Björn
//! Ottosson's published matrices, implemented by the `color` library.
//!
//! A reply never needs a block to show a color: [`color_ranges`] finds the
//! color values a model writes in prose, table cells, or inline code, and
//! right-clicking one offers [`color_copy_entries`]: the value as written,
//! then the color in every notation.

use std::ops::Range;

use super::super::actions::CopyEntry;

/// A parsed color: gamma-encoded sRGB channels (unclamped) and alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Color {
    pub(super) r: f64,
    pub(super) g: f64,
    pub(super) b: f64,
    pub(super) alpha: f64,
}

/// The notations a color can be shown and copied in.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Notation {
    Hex,
    Rgb,
    Hsl,
    Oklch,
    Oklab,
    P3,
}

impl Notation {
    /// Every notation, in switcher order.
    pub(super) const ALL: [Self; 6] = [
        Self::Hex,
        Self::Rgb,
        Self::Hsl,
        Self::Oklch,
        Self::Oklab,
        Self::P3,
    ];

    /// The switcher label.
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Hex => "HEX",
            Self::Rgb => "RGB",
            Self::Hsl => "HSL",
            Self::Oklch => "OKLCH",
            Self::Oklab => "OKLab",
            Self::P3 => "Display P3",
        }
    }
}

/// Rounding slack when testing channel edges.
const GAMUT_EPSILON: f64 = 1e-4;

impl Color {
    const fn srgb(r: f64, g: f64, b: f64, alpha: f64) -> Self {
        Self { r, g, b, alpha }
    }

    /// The color clipped into sRGB, as a screen can show it.
    pub(super) fn clipped(self) -> Self {
        Self::srgb(
            self.r.clamp(0.0, 1.0),
            self.g.clamp(0.0, 1.0),
            self.b.clamp(0.0, 1.0),
            self.alpha.clamp(0.0, 1.0),
        )
    }

    fn converted<Space: color::ColorSpace>(self) -> [f64; 3] {
        let value = color::AlphaColor::<color::Srgb>::new([
            self.r as f32,
            self.g as f32,
            self.b as f32,
            self.alpha as f32,
        ])
        .convert::<Space>()
        .components;
        [
            f64::from(value[0]),
            f64::from(value[1]),
            f64::from(value[2]),
        ]
    }
    fn to_p3(self) -> [f64; 3] {
        self.converted::<color::DisplayP3>()
    }
    pub(super) fn to_oklab(self) -> [f64; 3] {
        self.converted::<color::Oklab>()
    }
    pub(super) fn to_oklch(self) -> [f64; 3] {
        self.converted::<color::Oklch>()
    }
    pub(super) fn to_hsl(self) -> [f64; 3] {
        let [h, s, l] = self.clipped().converted::<color::Hsl>();
        [h, s / 100.0, l / 100.0]
    }

    /// The color written in `notation`.
    pub(super) fn format(self, notation: Notation) -> String {
        let alpha = |separator: &str| {
            if self.alpha < 1.0 - GAMUT_EPSILON {
                format!("{separator}{}", trim(self.alpha, 3))
            } else {
                String::new()
            }
        };
        match notation {
            Notation::Hex => {
                let c = self.clipped();
                let byte = |value: f64| {
                    #[expect(
                        clippy::cast_possible_truncation,
                        clippy::cast_sign_loss,
                        reason = "a clipped 0..=1 channel scaled and rounded into 0..=255"
                    )]
                    let byte = (value * 255.0).round() as u8;
                    format!("{byte:02x}")
                };
                let mut hex = format!("#{}{}{}", byte(c.r), byte(c.g), byte(c.b));
                if c.alpha < 1.0 - GAMUT_EPSILON {
                    hex.push_str(&byte(c.alpha));
                }
                hex
            }
            Notation::Rgb => {
                let c = self.clipped();
                format!(
                    "rgb({} {} {}{})",
                    trim(c.r * 255.0, 0),
                    trim(c.g * 255.0, 0),
                    trim(c.b * 255.0, 0),
                    alpha(" / ")
                )
            }
            Notation::Hsl => {
                let [h, s, l] = self.to_hsl();
                format!(
                    "hsl({} {}% {}%{})",
                    trim(h, 0),
                    trim(s * 100.0, 0),
                    trim(l * 100.0, 0),
                    alpha(" / ")
                )
            }
            Notation::Oklch => {
                let [l, c, h] = self.to_oklch();
                format!(
                    "oklch({} {} {}{})",
                    trim(l, 3),
                    trim(c, 3),
                    trim(h, 1),
                    alpha(" / ")
                )
            }
            Notation::Oklab => {
                let [l, a, b] = self.to_oklab();
                format!(
                    "oklab({} {} {}{})",
                    trim(l, 3),
                    trim(a, 3),
                    trim(b, 3),
                    alpha(" / ")
                )
            }
            Notation::P3 => {
                let [r, g, b] = self.to_p3();
                format!(
                    "color(display-p3 {} {} {}{})",
                    trim(r, 3),
                    trim(g, 3),
                    trim(b, 3),
                    alpha(" / ")
                )
            }
        }
    }
}

/// CSS Color 4 grammar and conversion are owned by Linebender's `color`.
pub(super) fn parse(text: &str) -> Option<Color> {
    let parsed: color::AlphaColor<color::Srgb> = text.trim().parse().ok()?;
    let [r, g, b, alpha] = parsed.components;
    [r, g, b, alpha]
        .iter()
        .all(|value| value.is_finite())
        .then(|| Color::srgb(f64::from(r), f64::from(g), f64::from(b), f64::from(alpha)))
}

/// `value` with at most `places` decimals and no trailing zeros.
fn trim(value: f64, places: usize) -> String {
    let text = format!("{value:.places$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    };
    if text == "-0" { "0".to_owned() } else { text }
}

/// CSS color functions a reply may write, lowercase.
const COLOR_FUNCTIONS: [&str; 10] = [
    "rgb", "rgba", "hsl", "hsla", "hwb", "lab", "lch", "oklab", "oklch", "color",
];

/// The longest color function call recognized, in bytes.
const MAX_FUNCTION_BYTES: usize = 96;

/// The color values in `text`, in order: hex values and CSS color
/// functions that parse. `code` lists the inline-code ranges: inside one,
/// any hex length counts; in prose a hex value needs six or eight digits or
/// a letter, so `#123`, which is far more often an issue number, stays text.
pub(in crate::markdown_renderer) fn color_ranges(
    text: &str,
    code: &[Range<usize>],
) -> Vec<Range<usize>> {
    let bytes = text.as_bytes();
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    let mut found = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let after_word = at > 0 && (is_word(bytes[at - 1]) || bytes[at - 1] == b'&');
        if bytes[at] == b'#' && !after_word {
            let digits = bytes[at + 1..]
                .iter()
                .take_while(|byte| byte.is_ascii_hexdigit())
                .count();
            let end = at + 1 + digits;
            let bounded = bytes.get(end).is_none_or(|byte| !is_word(*byte));
            let in_code = code.iter().any(|range| range.contains(&at));
            let letters = bytes[at + 1..end].iter().any(u8::is_ascii_alphabetic);
            let plausible =
                matches!(digits, 6 | 8) || (matches!(digits, 3 | 4) && (in_code || letters));
            if bounded && plausible && parse(&text[at..end]).is_some() {
                found.push(at..end);
            }
            at = end.max(at + 1);
            continue;
        }
        if bytes[at].is_ascii_alphabetic() && !after_word {
            let name_end = at
                + bytes[at..]
                    .iter()
                    .take_while(|byte| byte.is_ascii_alphabetic() || **byte == b'-')
                    .count();
            let name = text[at..name_end].to_ascii_lowercase();
            if bytes.get(name_end) == Some(&b'(') && COLOR_FUNCTIONS.contains(&name.as_str()) {
                let close = bytes[name_end..]
                    .iter()
                    .take(MAX_FUNCTION_BYTES)
                    .take_while(|byte| **byte != b'\n')
                    .position(|byte| *byte == b')')
                    .map(|offset| name_end + offset + 1);
                if let Some(end) = close
                    && parse(&text[at..end]).is_some()
                {
                    found.push(at..end);
                    at = end;
                    continue;
                }
            }
            at = name_end;
            continue;
        }
        at += 1;
    }
    found
}

/// The color `value` paints with, clipped into sRGB.
pub(in crate::markdown_renderer) fn color_paint(value: &str) -> Option<gpui::Hsla> {
    let c = parse(value)?.clipped();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clipped 0..=1 channels narrowed to the f32 GPUI paints"
    )]
    Some(gpui::rgb_to_hsla(gpui::Rgba::new(
        c.r as f32,
        c.g as f32,
        c.b as f32,
        c.alpha as f32,
    )))
}

/// The copy menu of one color value: the value as written, then the color
/// in every notation. A value that does not parse offers only itself.
pub(in crate::markdown_renderer) fn color_copy_entries(written: &str) -> Vec<CopyEntry> {
    let written = written.trim();
    let mut entries = vec![CopyEntry::primary("Copy Value", written.to_owned())];
    if let Some(color) = parse(written) {
        entries.extend(Notation::ALL.iter().map(|notation| {
            CopyEntry::alternative(
                format!("Copy as {}", notation.label()),
                color.format(*notation),
            )
        }));
    }
    entries
}
