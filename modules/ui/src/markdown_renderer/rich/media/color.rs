//! CSS colors: parsing every common notation, converting between spaces,
//! formatting in the notation the reader picks, and checking gamuts.
//!
//! A color is held as unclamped gamma-encoded sRGB plus alpha, so a wide
//! gamut color (Display P3, a vivid OKLCH) survives a round trip and its
//! distance outside sRGB can be reported. Conversions follow CSS Color 4:
//! sRGB and Display P3 through linear light and CIE XYZ (D65), Lab and LCH
//! through XYZ D50 with the Bradford adaptation, and `OKLab` through Björn
//! Ottosson's published matrices.
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

    fn linear(self) -> [f64; 3] {
        [self.r, self.g, self.b].map(to_linear)
    }

    fn from_linear(linear: [f64; 3], alpha: f64) -> Self {
        let [r, g, b] = linear.map(from_linear);
        Self::srgb(r, g, b, alpha)
    }

    fn to_xyz65(self) -> [f64; 3] {
        multiply(&SRGB_TO_XYZ65, self.linear())
    }

    fn from_xyz65(xyz: [f64; 3], alpha: f64) -> Self {
        Self::from_linear(multiply(&XYZ65_TO_SRGB, xyz), alpha)
    }

    fn to_p3(self) -> [f64; 3] {
        multiply(&XYZ65_TO_P3, self.to_xyz65()).map(from_linear)
    }

    /// `OKLab` `[L, a, b]`.
    pub(super) fn to_oklab(self) -> [f64; 3] {
        let lms = multiply(&SRGB_TO_LMS, self.linear()).map(f64::cbrt);
        multiply(&LMS_TO_OKLAB, lms)
    }

    fn from_oklab(lab: [f64; 3], alpha: f64) -> Self {
        let lms = multiply(&OKLAB_TO_LMS, lab).map(|value| value * value * value);
        Self::from_linear(multiply(&LMS_TO_SRGB, lms), alpha)
    }

    /// OKLCH `[L, C, h]`, hue in degrees.
    pub(super) fn to_oklch(self) -> [f64; 3] {
        polar(self.to_oklab())
    }

    /// HSL `[h, s, l]`: hue in degrees, saturation and lightness `0..=1`.
    pub(super) fn to_hsl(self) -> [f64; 3] {
        let c = self.clipped();
        let max = c.r.max(c.g).max(c.b);
        let min = c.r.min(c.g).min(c.b);
        let lightness = f64::midpoint(max, min);
        let delta = max - min;
        if delta <= f64::EPSILON {
            return [0.0, 0.0, lightness];
        }
        let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
        let hue = if (max - c.r).abs() <= f64::EPSILON {
            ((c.g - c.b) / delta).rem_euclid(6.0)
        } else if (max - c.g).abs() <= f64::EPSILON {
            (c.b - c.r) / delta + 2.0
        } else {
            (c.r - c.g) / delta + 4.0
        };
        [hue * 60.0, saturation, lightness]
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

/// Parses a CSS color; `None` when the text is not one this reads.
#[expect(
    clippy::many_single_char_names,
    reason = "color channels read best by their letters"
)]
pub(super) fn parse(text: &str) -> Option<Color> {
    let text = text.trim().to_ascii_lowercase();
    if let Some(hex) = text.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(color) = named(&text) {
        return Some(color);
    }
    let (name, args) = text.strip_suffix(')')?.split_once('(')?;
    let name = name.trim();
    if name == "color" {
        return parse_color_function(args);
    }
    let (channels, alpha) = arguments(args)?;
    let alpha = match alpha {
        Some(alpha) => number_or_percent(alpha, 1.0)?.clamp(0.0, 1.0),
        None => 1.0,
    };
    match (name, channels.as_slice()) {
        ("rgb" | "rgba", [r, g, b]) => Some(Color::srgb(
            number_or_percent(r, 255.0)? / 255.0,
            number_or_percent(g, 255.0)? / 255.0,
            number_or_percent(b, 255.0)? / 255.0,
            alpha,
        )),
        ("hsl" | "hsla", [h, s, l]) => {
            let [r, g, b] = hsl_to_srgb(
                hue(h)?,
                number_or_percent(s, 100.0)? / 100.0,
                number_or_percent(l, 100.0)? / 100.0,
            );
            Some(Color::srgb(r, g, b, alpha))
        }
        ("hwb", [h, w, b]) => {
            let (white, black) = (
                number_or_percent(w, 100.0)? / 100.0,
                number_or_percent(b, 100.0)? / 100.0,
            );
            Some(hwb_to_color(hue(h)?, white, black, alpha))
        }
        ("oklab", [l, a, b]) => Some(Color::from_oklab(
            [
                number_or_percent(l, 1.0)?,
                number_or_percent(a, 0.4)?,
                number_or_percent(b, 0.4)?,
            ],
            alpha,
        )),
        ("oklch", [l, c, h]) => Some(Color::from_oklab(
            cartesian([
                number_or_percent(l, 1.0)?,
                number_or_percent(c, 0.4)?.max(0.0),
                hue(h)?,
            ]),
            alpha,
        )),
        ("lab", [l, a, b]) => Some(lab_to_color(
            [
                number_or_percent(l, 100.0)?,
                number_or_percent(a, 125.0)?,
                number_or_percent(b, 125.0)?,
            ],
            alpha,
        )),
        ("lch", [l, c, h]) => Some(lab_to_color(
            cartesian([
                number_or_percent(l, 100.0)?,
                number_or_percent(c, 150.0)?.max(0.0),
                hue(h)?,
            ]),
            alpha,
        )),
        _ => None,
    }
}

/// `color(<space> c1 c2 c3 [/ a])` for `srgb`, `srgb-linear`, and
/// `display-p3`.
fn parse_color_function(args: &str) -> Option<Color> {
    let args = args.trim();
    let (space, rest) = args.split_once(char::is_whitespace)?;
    let (channels, alpha) = arguments(rest)?;
    let alpha = match alpha {
        Some(alpha) => number_or_percent(alpha, 1.0)?.clamp(0.0, 1.0),
        None => 1.0,
    };
    let [a, b, c] = [channels.first()?, channels.get(1)?, channels.get(2)?]
        .map(|channel| number_or_percent(channel, 1.0));
    let channels = [a?, b?, c?];
    match space {
        "srgb" => Some(Color::srgb(channels[0], channels[1], channels[2], alpha)),
        "srgb-linear" => Some(Color::from_linear(channels, alpha)),
        "display-p3" => Some(Color::from_xyz65(
            multiply(&P3_TO_XYZ65, channels.map(to_linear)),
            alpha,
        )),
        _ => None,
    }
}

/// Splits `a b c / d`, `a, b, c`, or `a, b, c, d` into channels and alpha.
fn arguments(args: &str) -> Option<(Vec<&str>, Option<&str>)> {
    let (channels, alpha) = match args.split_once('/') {
        Some((channels, alpha)) => (channels, Some(alpha.trim())),
        None => (args, None),
    };
    let mut channels: Vec<&str> = channels
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect();
    match (channels.len(), alpha) {
        (3, _) => Some((channels, alpha)),
        (4, None) => {
            let alpha = channels.pop();
            Some((channels, alpha))
        }
        _ => None,
    }
}

/// A number, or a percentage of `full`. `none` reads as zero, as in CSS.
fn number_or_percent(text: &str, full: f64) -> Option<f64> {
    let text = text.trim();
    if text == "none" {
        return Some(0.0);
    }
    let value = match text.strip_suffix('%') {
        Some(percent) => percent.trim().parse::<f64>().ok()? / 100.0 * full,
        None => text.parse::<f64>().ok()?,
    };
    value.is_finite().then_some(value)
}

/// A hue in degrees from `deg`, `rad`, `grad`, `turn`, or a bare number.
fn hue(text: &str) -> Option<f64> {
    let text = text.trim();
    if text == "none" {
        return Some(0.0);
    }
    let (number, scale) = if let Some(n) = text.strip_suffix("deg") {
        (n, 1.0)
    } else if let Some(n) = text.strip_suffix("grad") {
        (n, 0.9)
    } else if let Some(n) = text.strip_suffix("rad") {
        (n, 180.0 / std::f64::consts::PI)
    } else if let Some(n) = text.strip_suffix("turn") {
        (n, 360.0)
    } else {
        (text, 1.0)
    };
    let value = number.trim().parse::<f64>().ok()? * scale;
    value.is_finite().then(|| value.rem_euclid(360.0))
}

fn parse_hex(hex: &str) -> Option<Color> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let digits: Vec<f64> = hex
        .chars()
        .filter_map(|c| c.to_digit(16))
        .map(f64::from)
        .collect();
    let channels: Vec<f64> = match digits.len() {
        3 | 4 => digits.iter().map(|digit| digit * 17.0 / 255.0).collect(),
        6 | 8 => digits
            .chunks(2)
            .map(|pair| (pair[0] * 16.0 + pair[1]) / 255.0)
            .collect(),
        _ => return None,
    };
    let channel = |index: usize| channels.get(index).copied().unwrap_or(1.0);
    Some(Color::srgb(channel(0), channel(1), channel(2), channel(3)))
}

fn hsl_to_srgb(hue: f64, saturation: f64, lightness: f64) -> [f64; 3] {
    let (s, l) = (saturation.clamp(0.0, 1.0), lightness.clamp(0.0, 1.0));
    let f = |n: f64| {
        let k = (n + hue / 30.0).rem_euclid(12.0);
        let a = s * l.min(1.0 - l);
        l - a * (k - 3.0).min(9.0 - k).clamp(-1.0, 1.0)
    };
    [f(0.0), f(8.0), f(4.0)]
}

fn hwb_to_color(hue: f64, white: f64, black: f64, alpha: f64) -> Color {
    let (white, black) = (white.clamp(0.0, 1.0), black.clamp(0.0, 1.0));
    if white + black >= 1.0 {
        let gray = white / (white + black);
        return Color::srgb(gray, gray, gray, alpha);
    }
    let [r, g, b] =
        hsl_to_srgb(hue, 1.0, 0.5).map(|channel| channel * (1.0 - white - black) + white);
    Color::srgb(r, g, b, alpha)
}

/// CIE Lab (D50) to a color, through XYZ D50 and the Bradford adaptation.
fn lab_to_color([l, a, b]: [f64; 3], alpha: f64) -> Color {
    const KAPPA: f64 = 24_389.0 / 27.0;
    const EPSILON: f64 = 216.0 / 24_389.0;
    let fy = (l + 16.0) / 116.0;
    let fx = a / 500.0 + fy;
    let fz = fy - b / 200.0;
    let inverse = |f: f64| {
        let cube = f * f * f;
        if cube > EPSILON {
            cube
        } else {
            (116.0 * f - 16.0) / KAPPA
        }
    };
    let y = if l > KAPPA * EPSILON {
        fy * fy * fy
    } else {
        l / KAPPA
    };
    let xyz50 = [inverse(fx) * D50[0], y * D50[1], inverse(fz) * D50[2]];
    Color::from_xyz65(multiply(&D50_TO_D65, xyz50), alpha)
}

/// `[L, C, h°]` from a Cartesian `[L, a, b]`.
fn polar([l, a, b]: [f64; 3]) -> [f64; 3] {
    let chroma = a.hypot(b);
    let hue = if chroma < 1e-6 {
        0.0
    } else {
        b.atan2(a).to_degrees().rem_euclid(360.0)
    };
    [l, chroma, hue]
}

/// `[L, a, b]` from a polar `[L, C, h°]`.
fn cartesian([l, c, h]: [f64; 3]) -> [f64; 3] {
    let radians = h.to_radians();
    [l, c * radians.cos(), c * radians.sin()]
}

fn to_linear(channel: f64) -> f64 {
    let magnitude = channel.abs();
    let linear = if magnitude <= 0.040_45 {
        magnitude / 12.92
    } else {
        ((magnitude + 0.055) / 1.055).powf(2.4)
    };
    linear.copysign(channel)
}

fn from_linear(channel: f64) -> f64 {
    let magnitude = channel.abs();
    let encoded = if magnitude <= 0.003_130_8 {
        magnitude * 12.92
    } else {
        1.055 * magnitude.powf(1.0 / 2.4) - 0.055
    };
    encoded.copysign(channel)
}

fn multiply(matrix: &[[f64; 3]; 3], vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| row[0] * vector[0] + row[1] * vector[1] + row[2] * vector[2])
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

const D50: [f64; 3] = [0.964_295_676, 1.0, 0.825_104_603];

const SRGB_TO_XYZ65: [[f64; 3]; 3] = [
    [
        0.412_390_799_265_959_5,
        0.357_584_339_383_878,
        0.180_480_788_401_834_3,
    ],
    [
        0.212_639_005_871_510_27,
        0.715_168_678_767_756,
        0.072_192_315_360_733_71,
    ],
    [
        0.019_330_818_715_591_82,
        0.119_194_779_794_625_98,
        0.950_532_152_249_660_7,
    ],
];

const XYZ65_TO_SRGB: [[f64; 3]; 3] = [
    [
        3.240_969_941_904_521_3,
        -1.537_383_177_570_093_5,
        -0.498_610_760_293_003_3,
    ],
    [
        -0.969_243_636_280_879_6,
        1.875_967_501_507_720_7,
        0.041_555_057_407_175_61,
    ],
    [
        0.055_630_079_696_993_6,
        -0.203_976_958_888_976_6,
        1.056_971_514_242_878_6,
    ],
];

const P3_TO_XYZ65: [[f64; 3]; 3] = [
    [
        0.486_570_948_648_216_2,
        0.265_667_693_169_093_1,
        0.198_217_285_234_362_5,
    ],
    [
        0.228_974_564_069_748_8,
        0.691_738_521_836_506_4,
        0.079_286_914_093_745,
    ],
    [0.0, 0.045_113_381_858_902_63, 1.043_944_368_900_976],
];

const XYZ65_TO_P3: [[f64; 3]; 3] = [
    [
        2.493_496_911_941_425,
        -0.931_383_617_919_123_9,
        -0.402_710_784_450_716_8,
    ],
    [
        -0.829_488_969_561_574_7,
        1.762_664_060_318_346_3,
        0.023_624_685_841_943_577,
    ],
    [
        0.035_845_830_243_784_47,
        -0.076_172_389_268_041_6,
        0.956_884_524_007_687_2,
    ],
];

/// Bradford chromatic adaptation from D50 to D65.
const D50_TO_D65: [[f64; 3]; 3] = [
    [
        0.955_473_421_488_075,
        -0.023_098_454_948_764_08,
        0.063_259_243_200_570_4,
    ],
    [
        -0.028_369_709_333_863_7,
        1.009_995_398_081_304_2,
        0.021_041_441_191_917_323,
    ],
    [
        0.012_314_014_864_481_998,
        -0.020_507_649_298_898_964,
        1.330_365_926_242_124,
    ],
];

const SRGB_TO_LMS: [[f64; 3]; 3] = [
    [0.412_221_470_8, 0.536_332_536_3, 0.051_445_992_9],
    [0.211_903_498_2, 0.680_699_545_1, 0.107_396_956_6],
    [0.088_302_461_9, 0.281_718_837_6, 0.629_978_700_5],
];

const LMS_TO_OKLAB: [[f64; 3]; 3] = [
    [0.210_454_255_3, 0.793_617_785, -0.004_072_046_8],
    [1.977_998_495_1, -2.428_592_205, 0.450_593_709_9],
    [0.025_904_037_1, 0.782_771_766_2, -0.808_675_766],
];

const OKLAB_TO_LMS: [[f64; 3]; 3] = [
    [1.0, 0.396_337_777_4, 0.215_803_757_3],
    [1.0, -0.105_561_345_8, -0.063_854_172_8],
    [1.0, -0.089_484_177_5, -1.291_485_548],
];

const LMS_TO_SRGB: [[f64; 3]; 3] = [
    [4.076_741_662_1, -3.307_711_591_3, 0.230_969_929_2],
    [-1.268_438_004_6, 2.609_757_401_1, -0.341_319_396_5],
    [-0.004_196_086_3, -0.703_418_614_7, 1.707_614_701],
];

/// The CSS named colors most often written in design tokens.
fn named(name: &str) -> Option<Color> {
    let hex = match name {
        "transparent" => return Some(Color::srgb(0.0, 0.0, 0.0, 0.0)),
        "black" => "000000",
        "white" => "ffffff",
        "red" => "ff0000",
        "green" => "008000",
        "lime" => "00ff00",
        "blue" => "0000ff",
        "yellow" => "ffff00",
        "cyan" | "aqua" => "00ffff",
        "magenta" | "fuchsia" => "ff00ff",
        "gray" | "grey" => "808080",
        "silver" => "c0c0c0",
        "maroon" => "800000",
        "olive" => "808000",
        "purple" => "800080",
        "teal" => "008080",
        "navy" => "000080",
        "orange" => "ffa500",
        "pink" => "ffc0cb",
        "brown" => "a52a2a",
        "gold" => "ffd700",
        "indigo" => "4b0082",
        "violet" => "ee82ee",
        "coral" => "ff7f50",
        "salmon" => "fa8072",
        "tomato" => "ff6347",
        "crimson" => "dc143c",
        "turquoise" => "40e0d0",
        "tan" => "d2b48c",
        "beige" => "f5f5dc",
        "ivory" => "fffff0",
        "khaki" => "f0e68c",
        "lavender" => "e6e6fa",
        "plum" => "dda0dd",
        "orchid" => "da70d6",
        "chocolate" => "d2691e",
        "sienna" => "a0522d",
        "rebeccapurple" => "663399",
        "slategray" | "slategrey" => "708090",
        "darkgray" | "darkgrey" => "a9a9a9",
        "lightgray" | "lightgrey" => "d3d3d3",
        "dimgray" | "dimgrey" => "696969",
        "whitesmoke" => "f5f5f5",
        "gainsboro" => "dcdcdc",
        "skyblue" => "87ceeb",
        "steelblue" => "4682b4",
        "royalblue" => "4169e1",
        "dodgerblue" => "1e90ff",
        "deepskyblue" => "00bfff",
        "seagreen" => "2e8b57",
        "forestgreen" => "228b22",
        "limegreen" => "32cd32",
        "springgreen" => "00ff7f",
        "firebrick" => "b22222",
        "darkred" => "8b0000",
        "hotpink" => "ff69b4",
        "deeppink" => "ff1493",
        "goldenrod" => "daa520",
        "darkorange" => "ff8c00",
        _ => return None,
    };
    parse_hex(hex)
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
