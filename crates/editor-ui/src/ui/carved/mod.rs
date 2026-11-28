pub mod avatar;
pub mod button;
pub mod face;
pub mod glyph;
pub mod menu;
pub mod tabs;
pub mod well;

use crate::ui::theme::{ArtisanTheme, Oklch};
use aqua::ShadowLayer;
use aqua_text::{FontFamily, FontWeight, TextStyle};

pub const RADIUS_SM: f32 = 6.0;
pub const RADIUS_MD: f32 = 9.0;
pub const RADIUS_CARD: f32 = 16.0;
pub const RADIUS_SHELL: f32 = 20.0;

const WHITE: Oklch = Oklch::new(1.0, 0.0, 0.0);
const BLACK: Oklch = Oklch::new(0.0, 0.0, 0.0);
const GRAY_50: Oklch = Oklch::new(0.985, 0.002, 247.839);
const GRAY_100: Oklch = Oklch::new(0.967, 0.003, 264.542);
const GRAY_200: Oklch = Oklch::new(0.928, 0.006, 264.531);
const GRAY_300: Oklch = Oklch::new(0.872, 0.01, 258.338);
const GRAY_600: Oklch = Oklch::new(0.446, 0.03, 256.802);
const GRAY_900: Oklch = Oklch::new(0.21, 0.034, 264.665);
const POLAR_300: Oklch = Oklch::new(0.709, 0.0, 0.0);
const POLAR_400: Oklch = Oklch::new(0.6154, 0.0, 0.0);
const POLAR_600: Oklch = Oklch::new(0.2801, 0.0, 0.0);
const POLAR_700: Oklch = Oklch::new(0.2484, 0.0, 0.0);
const POLAR_800: Oklch = Oklch::new(0.21, 0.0, 0.0);
const POLAR_900: Oklch = Oklch::new(0.1757, 0.0, 0.0);
const POLAR_950: Oklch = Oklch::new(0.1394, 0.0, 0.0);
const NIGHT_TINT: Oklch = Oklch::new(0.1, 0.03, 265.0);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Carve {
    pub dark: bool,
    pub ink: Oklch,
    pub ink_dim: Oklch,
    pub ink_faint: Oklch,
    pub frame: Oklch,
    pub bg: Oklch,
    pub bg_raised: Oklch,
    pub line: Oklch,
    pub line_soft: Oklch,
    pub card: Oklch,
    pub well: Oklch,
    pub tint: Oklch,
    pub rule_shade: Oklch,
    pub rule_light: Oklch,
    pub ok: Oklch,
    pub warn: Oklch,
    pub err: Oklch,
}

impl Carve {
    #[must_use]
    pub fn of(theme: &ArtisanTheme) -> Self {
        if theme.colors.mode.is_dark() {
            Self {
                dark: true,
                ink: WHITE,
                ink_dim: POLAR_300,
                ink_faint: POLAR_400,
                frame: POLAR_950,
                bg: POLAR_900,
                bg_raised: POLAR_800,
                line: POLAR_700,
                line_soft: POLAR_600,
                card: mix(POLAR_800, WHITE, 0.06),
                well: POLAR_900,
                tint: NIGHT_TINT,
                rule_shade: BLACK.with_alpha(0.55),
                rule_light: WHITE.with_alpha(0.07),
                ok: Oklch::new(0.696, 0.17, 162.48),
                warn: Oklch::new(0.769, 0.188, 70.08),
                err: Oklch::new(0.704, 0.191, 22.216),
            }
        } else {
            Self {
                dark: false,
                ink: BLACK,
                ink_dim: GRAY_600,
                ink_faint: GRAY_600,
                frame: GRAY_100,
                bg: WHITE,
                bg_raised: GRAY_50,
                line: GRAY_200,
                line_soft: GRAY_300,
                card: GRAY_100,
                well: GRAY_200,
                tint: GRAY_900,
                rule_shade: GRAY_900.with_alpha(0.08),
                rule_light: WHITE.with_alpha(0.9),
                ok: Oklch::new(0.45, 0.13, 163.0),
                warn: Oklch::new(0.48, 0.14, 58.0),
                err: Oklch::new(0.505, 0.213, 27.518),
            }
        }
    }

    #[must_use]
    pub fn rest(&self) -> Vec<ShadowLayer> {
        let alphas = if self.dark {
            [0.12, 0.10, 0.08, 0.06, 0.04]
        } else {
            [0.06, 0.05, 0.04, 0.03, 0.02]
        };
        [(0.5, 1.0), (1.0, 2.0), (2.0, 4.0), (4.0, 8.0), (8.0, 16.0)]
            .into_iter()
            .zip(alphas)
            .map(|((dy, blur), alpha)| drop_shadow(dy, blur, 0.0, self.tint.with_alpha(alpha)))
            .collect()
    }

    #[must_use]
    pub(crate) fn press(&self) -> Vec<ShadowLayer> {
        let alphas = if self.dark {
            [0.16, 0.10]
        } else {
            [0.08, 0.05]
        };
        [(0.5, 1.0), (1.0, 2.0)]
            .into_iter()
            .zip(alphas)
            .map(|((dy, blur), alpha)| drop_shadow(dy, blur, 0.0, self.tint.with_alpha(alpha)))
            .collect()
    }

    #[must_use]
    pub fn overlay(&self) -> Vec<ShadowLayer> {
        let casts: [(f32, f32, f32); 4] = if self.dark {
            [
                (1.0, 2.0, 0.12),
                (4.0, 8.0, 0.12),
                (12.0, 24.0, 0.12),
                (28.0, 56.0, 0.10),
            ]
        } else {
            [
                (1.0, 2.0, 0.04),
                (4.0, 8.0, 0.04),
                (16.0, 32.0, 0.05),
                (32.0, 64.0, 0.04),
            ]
        };
        casts
            .into_iter()
            .map(|(dy, blur, alpha)| drop_shadow(dy, blur, 0.0, self.tint.with_alpha(alpha)))
            .collect()
    }

    #[must_use]
    pub fn inset(&self) -> Vec<ShadowLayer> {
        let (deep, light) = if self.dark {
            ([0.18, 0.12, 0.08], WHITE.with_alpha(0.08))
        } else {
            ([0.08, 0.06, 0.04], WHITE.with_alpha(0.9))
        };
        vec![
            inner_shadow(0.0, 1.0, 1.0, -0.5, self.tint.with_alpha(deep[0])),
            inner_shadow(0.0, 3.0, 3.0, -1.5, self.tint.with_alpha(deep[1])),
            inner_shadow(0.0, 6.0, 6.0, -3.0, self.tint.with_alpha(deep[2])),
            inner_shadow(0.0, -0.5, 0.0, 0.0, light),
            inner_shadow(0.0, 0.0, 0.0, 0.5, self.ink.with_alpha(0.08)),
        ]
    }

    #[must_use]
    pub(crate) fn toward_ink(&self, base: Oklch, amount: f32) -> Oklch {
        mix(base, self.ink, amount)
    }
}

#[must_use]
pub(crate) fn mix(from: Oklch, to: Oklch, amount: f32) -> Oklch {
    from.animate_to(to, amount)
}

#[must_use]
pub(crate) fn black(alpha: f32) -> Oklch {
    BLACK.with_alpha(alpha)
}

#[must_use]
pub(crate) fn white(alpha: f32) -> Oklch {
    WHITE.with_alpha(alpha)
}

fn layer(dx: f32, dy: f32, blur: f32, spread: f32, inset: bool, color: Oklch) -> ShadowLayer {
    ShadowLayer {
        dx,
        dy,
        blur,
        spread,
        inset,
        color: color.to_paint(),
    }
}

#[must_use]
pub(crate) fn drop_shadow(dy: f32, blur: f32, spread: f32, color: Oklch) -> ShadowLayer {
    layer(0.0, dy, blur, spread, false, color)
}

#[must_use]
pub(crate) fn inner_shadow(dx: f32, dy: f32, blur: f32, spread: f32, color: Oklch) -> ShadowLayer {
    layer(dx, dy, blur, spread, true, color)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Type {
    pub(crate) size: f32,
    pub(crate) line: f32,
    pub(crate) weight: FontWeight,
    pub(crate) tracking: f32,
    pub(crate) mono: bool,
}

impl Type {
    #[must_use]
    pub const fn sans(size: f32, line: f32, weight: u16) -> Self {
        Self {
            size,
            line,
            weight: FontWeight(weight),
            tracking: -0.04,
            mono: false,
        }
    }

    #[must_use]
    pub const fn heading(size: f32, line: f32, weight: u16) -> Self {
        Self {
            size,
            line,
            weight: FontWeight(weight),
            tracking: -0.045,
            mono: false,
        }
    }

    #[must_use]
    pub const fn mono(size: f32, line: f32, weight: u16) -> Self {
        Self {
            size,
            line,
            weight: FontWeight(weight),
            tracking: 0.0,
            mono: true,
        }
    }

    #[must_use]
    pub const fn untracked(mut self) -> Self {
        self.tracking = 0.0;
        self
    }

    #[must_use]
    pub fn style(self, theme: &ArtisanTheme) -> TextStyle {
        let family = if self.mono {
            theme.typography.mono.family
        } else {
            theme.typography.sans.family
        };
        let tracks = theme.typography.tracking_em != 0.0;
        TextStyle {
            families: vec![FontFamily::Named(family.to_owned())],
            size_px: theme.typography.size(self.size),
            line_height_px: theme.typography.size(self.line),
            weight: self.weight,
            letter_spacing_em: (tracks && self.tracking != 0.0).then_some(self.tracking),
            ..TextStyle::default()
        }
    }

    #[must_use]
    pub fn text(
        self,
        theme: &ArtisanTheme,
        color: Oklch,
        words: impl aqua_ui::element::TextSource,
    ) -> aqua_ui::element::Text {
        aqua_ui::element::Text::new(words)
            .text_style(self.style(theme))
            .gpui_source_color(color.to_reference_text_color())
    }
}
