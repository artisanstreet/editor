//! Exact typed visual-theme foundation for the shared `GPUI` framework.
//!
//! Every value is transcribed from the audited legacy style layer
//! (`modules/frontend/src/lib/styles/{theme,fonts,global,utilities}.css`) and
//! `docs/ui/INVENTORY.md` §5. OKLCH source values stay first-party typed
//! data; conversion into a GPUI paint color is explicit and tested. This
//! module defines values only: no globals, observers, widgets, or frontend
//! wiring.

use gpui::{Font, FontWeight, Hsla, Pixels, hsla, px};

/// One encoded/display sRGB component triple plus alpha, all `[0, 1]`.
///
/// Values are post-transfer-function (gamma-encoded) sRGB ready for display;
/// the linear-sRGB stage of the conversion below never escapes this function.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SrgbComponents {
    /// Red channel.
    pub r: f32,
    /// Green channel.
    pub g: f32,
    /// Blue channel.
    pub b: f32,
    /// Alpha channel.
    pub a: f32,
}

/// A legacy `oklch(L C H / A)` source color, kept verbatim.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Oklch {
    /// Perceptual lightness, `0..=1`.
    pub l: f32,
    /// Chroma, `>= 0`.
    pub c: f32,
    /// Hue angle in degrees.
    pub h: f32,
    /// Alpha, `0..=1`.
    pub a: f32,
}

impl Oklch {
    /// An opaque source color from the exact legacy literals.
    #[must_use]
    pub const fn new(l: f32, c: f32, h: f32) -> Self {
        Self { l, c, h, a: 1.0 }
    }

    /// The same color at an explicit alpha (`oklch(.. / A)`).
    #[must_use]
    pub const fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        self
    }

    /// Converts through CSS Color 4 matrices into display-ready sRGB.
    ///
    /// Internal math runs in `f64`; out-of-gamut channels clamp at the final
    /// encoded sRGB boundary only.
    #[must_use]
    pub fn to_srgb(self) -> SrgbComponents {
        let radians = f64::from(self.h) * std::f64::consts::TAU / 360.0;
        let a = f64::from(self.c) * radians.cos();
        let b = f64::from(self.c) * radians.sin();
        let l = f64::from(self.l);

        // Björn Ottosson's OKLab → linear-sRGB constants (CSS Color 4 §4).
        let l_ = l + 0.396_337_777_4 * a + 0.215_803_757_3 * b;
        let m_ = l - 0.105_561_345_8 * a - 0.063_854_172_8 * b;
        let s_ = l - 0.089_484_177_5 * a - 1.291_485_548_0 * b;
        let cube = |v: f64| v * v * v;
        let (big_l, big_m, big_s) = (cube(l_), cube(m_), cube(s_));

        let lin_r = 4.076_741_662_1 * big_l - 3.307_711_591_3 * big_m + 0.230_969_929_2 * big_s;
        let lin_g = -1.268_438_004_6 * big_l + 2.609_757_401_1 * big_m - 0.341_319_396_5 * big_s;
        let lin_b = -0.004_196_086_3 * big_l - 0.703_418_614_7 * big_m + 1.707_614_701_0 * big_s;

        SrgbComponents {
            r: encode(lin_r),
            g: encode(lin_g),
            b: encode(lin_b),
            a: self.a,
        }
    }

    /// Builds the GPUI paint color for this source value.
    ///
    /// The clamped sRGB triple is converted with the standard sRGB→HSL math;
    /// that mapping is bijective for in-gamut colors, so GPUI's own paint-time
    /// HSL→RGB reproduces the pinned sRGB within float precision.
    #[must_use]
    pub fn to_paint(self) -> Hsla {
        srgb_to_hsla(self.to_srgb())
    }
}

/// sRGB transfer function (CSS Color 4); negative linear values keep their
/// sign and everything clamps to the `[0, 1]` gamut boundary at the end.
///
/// The final narrowing is deliberate: after clamping, the value lies in
/// `[0, 1]` and the f64→f32 rounding (~1e-7 relative) sits far below the
/// conversion tolerances pinned by the external tests.
#[allow(clippy::cast_possible_truncation)]
fn encode(linear: f64) -> f32 {
    let magnitude = linear.abs();
    let encoded = if magnitude <= 0.003_130_8 {
        12.92 * magnitude
    } else {
        1.055 * magnitude.powf(1.0 / 2.4) - 0.055
    };
    let signed = if linear < 0.0 { -encoded } else { encoded };
    signed.clamp(0.0, 1.0) as f32
}

/// Standard sRGB→HSL over an already-clamped triple.
///
/// The equality tests are structural, not tolerance comparisons: the three
/// channels come from the same computation over identical inputs, so equal
/// channels always compare bit-exact and `max == channel` selects the hue
/// sector deterministically.
#[allow(clippy::float_cmp)]
fn srgb_to_hsla(c: SrgbComponents) -> Hsla {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let delta = max - min;
    let lightness = max.midpoint(min);

    if delta == 0.0 {
        return hsla(0.0, 0.0, lightness, c.a);
    }
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs());
    let hue_sixth = if max == c.r {
        ((c.g - c.b) / delta).rem_euclid(6.0)
    } else if max == c.g {
        (c.b - c.r) / delta + 2.0
    } else {
        (c.r - c.g) / delta + 4.0
    };
    hsla(hue_sixth / 6.0, saturation, lightness, c.a)
}

/// The legacy neutral surface ramp, one variant per `--surface-*` token
/// (`theme.css:49–89`; INVENTORY §5.1 "41-step neutral oklch surface ramp").
///
/// Variant names prefix the legacy step number with `S` because Rust cannot
/// start an identifier with a digit: [`SurfaceStep::S250`] is `--surface-250`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SurfaceStep {
    /// `--surface-0` — `oklch(1 0 0)`.
    S0,
    /// `--surface-25` — `oklch(0.9925 0 0)`.
    S25,
    /// `--surface-50` — `oklch(0.985 0 0)`.
    S50,
    /// `--surface-75` — `oklch(0.976 0.0005 286.375)`.
    S75,
    /// `--surface-100` — `oklch(0.967 0.001 286.375)`.
    S100,
    /// `--surface-125` — `oklch(0.9552 0.0018 286.361)`.
    S125,
    /// `--surface-150` — `oklch(0.9435 0.0025 286.347)`.
    S150,
    /// `--surface-175` — `oklch(0.9318 0.0033 286.334)`.
    S175,
    /// `--surface-200` — `oklch(0.92 0.004 286.32)`.
    S200,
    /// `--surface-225` — `oklch(0.9078 0.0045 286.312)`.
    S225,
    /// `--surface-250` — `oklch(0.8955 0.005 286.303)`.
    S250,
    /// `--surface-275` — `oklch(0.8832 0.0055 286.294)`.
    S275,
    /// `--surface-300` — `oklch(0.871 0.006 286.286)`.
    S300,
    /// `--surface-325` — `oklch(0.8295 0.0083 286.231)`.
    S325,
    /// `--surface-350` — `oklch(0.788 0.0105 286.177)`.
    S350,
    /// `--surface-375` — `oklch(0.7465 0.0127 286.122)`.
    S375,
    /// `--surface-400` — `oklch(0.705 0.015 286.067)`.
    S400,
    /// `--surface-425` — `oklch(0.6667 0.0152 286.035)`.
    S425,
    /// `--surface-450` — `oklch(0.6285 0.0155 286.002)`.
    S450,
    /// `--surface-475` — `oklch(0.5903 0.0158 285.97)`.
    S475,
    /// `--surface-500` — `oklch(0.552 0.016 285.938)`.
    S500,
    /// `--surface-525` — `oklch(0.5245 0.0163 285.9)`.
    S525,
    /// `--surface-550` — `oklch(0.497 0.0165 285.862)`.
    S550,
    /// `--surface-575` — `oklch(0.4695 0.0168 285.824)`.
    S575,
    /// `--surface-600` — `oklch(0.442 0.017 285.786)`.
    S600,
    /// `--surface-625` — `oklch(0.424 0.016 285.791)`.
    S625,
    /// `--surface-650` — `oklch(0.406 0.015 285.796)`.
    S650,
    /// `--surface-675` — `oklch(0.388 0.014 285.8)`.
    S675,
    /// `--surface-700` — `oklch(0.37 0.013 285.805)`.
    S700,
    /// `--surface-725` — `oklch(0.346 0.0112 285.862)`.
    S725,
    /// `--surface-750` — `oklch(0.322 0.0095 285.919)`.
    S750,
    /// `--surface-775` — `oklch(0.298 0.0077 285.976)`.
    S775,
    /// `--surface-800` — `oklch(0.274 0.006 286.033)`.
    S800,
    /// `--surface-825` — `oklch(0.258 0.006 285.996)`.
    S825,
    /// `--surface-850` — `oklch(0.242 0.006 285.959)`.
    S850,
    /// `--surface-875` — `oklch(0.226 0.006 285.922)`.
    S875,
    /// `--surface-900` — `oklch(0.21 0.006 285.885)`.
    S900,
    /// `--surface-925` — `oklch(0.1755 0.0055 285.854)`.
    S925,
    /// `--surface-950` — `oklch(0.141 0.005 285.823)`.
    S950,
    /// `--surface-975` — `oklch(0.0705 0.0025 285.823)`.
    S975,
    /// `--surface-1000` — `oklch(0 0 0)`.
    S1000,
}

impl SurfaceStep {
    /// Every ramp step in ascending source order.
    pub const ALL: [SurfaceStep; 41] = [
        Self::S0,
        Self::S25,
        Self::S50,
        Self::S75,
        Self::S100,
        Self::S125,
        Self::S150,
        Self::S175,
        Self::S200,
        Self::S225,
        Self::S250,
        Self::S275,
        Self::S300,
        Self::S325,
        Self::S350,
        Self::S375,
        Self::S400,
        Self::S425,
        Self::S450,
        Self::S475,
        Self::S500,
        Self::S525,
        Self::S550,
        Self::S575,
        Self::S600,
        Self::S625,
        Self::S650,
        Self::S675,
        Self::S700,
        Self::S725,
        Self::S750,
        Self::S775,
        Self::S800,
        Self::S825,
        Self::S850,
        Self::S875,
        Self::S900,
        Self::S925,
        Self::S950,
        Self::S975,
        Self::S1000,
    ];

    /// The exact legacy OKLCH triple for this step.
    #[must_use]
    pub const fn oklch(self) -> Oklch {
        match self {
            Self::S0 => Oklch::new(1.0, 0.0, 0.0),
            Self::S25 => Oklch::new(0.9925, 0.0, 0.0),
            Self::S50 => Oklch::new(0.985, 0.0, 0.0),
            Self::S75 => Oklch::new(0.976, 0.0005, 286.375),
            Self::S100 => Oklch::new(0.967, 0.001, 286.375),
            Self::S125 => Oklch::new(0.9552, 0.0018, 286.361),
            Self::S150 => Oklch::new(0.9435, 0.0025, 286.347),
            Self::S175 => Oklch::new(0.9318, 0.0033, 286.334),
            Self::S200 => Oklch::new(0.92, 0.004, 286.32),
            Self::S225 => Oklch::new(0.9078, 0.0045, 286.312),
            Self::S250 => Oklch::new(0.8955, 0.005, 286.303),
            Self::S275 => Oklch::new(0.8832, 0.0055, 286.294),
            Self::S300 => Oklch::new(0.871, 0.006, 286.286),
            Self::S325 => Oklch::new(0.8295, 0.0083, 286.231),
            Self::S350 => Oklch::new(0.788, 0.0105, 286.177),
            Self::S375 => Oklch::new(0.7465, 0.0127, 286.122),
            Self::S400 => Oklch::new(0.705, 0.015, 286.067),
            Self::S425 => Oklch::new(0.6667, 0.0152, 286.035),
            Self::S450 => Oklch::new(0.6285, 0.0155, 286.002),
            Self::S475 => Oklch::new(0.5903, 0.0158, 285.97),
            Self::S500 => Oklch::new(0.552, 0.016, 285.938),
            Self::S525 => Oklch::new(0.5245, 0.0163, 285.9),
            Self::S550 => Oklch::new(0.497, 0.0165, 285.862),
            Self::S575 => Oklch::new(0.4695, 0.0168, 285.824),
            Self::S600 => Oklch::new(0.442, 0.017, 285.786),
            Self::S625 => Oklch::new(0.424, 0.016, 285.791),
            Self::S650 => Oklch::new(0.406, 0.015, 285.796),
            Self::S675 => Oklch::new(0.388, 0.014, 285.8),
            Self::S700 => Oklch::new(0.37, 0.013, 285.805),
            Self::S725 => Oklch::new(0.346, 0.0112, 285.862),
            Self::S750 => Oklch::new(0.322, 0.0095, 285.919),
            Self::S775 => Oklch::new(0.298, 0.0077, 285.976),
            Self::S800 => Oklch::new(0.274, 0.006, 286.033),
            Self::S825 => Oklch::new(0.258, 0.006, 285.996),
            Self::S850 => Oklch::new(0.242, 0.006, 285.959),
            Self::S875 => Oklch::new(0.226, 0.006, 285.922),
            Self::S900 => Oklch::new(0.21, 0.006, 285.885),
            Self::S925 => Oklch::new(0.1755, 0.0055, 285.854),
            Self::S950 => Oklch::new(0.141, 0.005, 285.823),
            Self::S975 => Oklch::new(0.0705, 0.0025, 285.823),
            Self::S1000 => Oklch::new(0.0, 0.0, 0.0),
        }
    }
}

/// Light or dark presentation.
///
/// The legacy default is **dark**: `<ModeWatcher defaultMode="dark" />`
/// (`routes/+layout.svelte:503`; INVENTORY §5.1). System resolution belongs to
/// the consumer, so no `System` variant exists here.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThemeMode {
    /// The legacy `.light` root token set (`theme.css:8–48`).
    Light,
    /// The legacy `.dark` token set and the app default (`theme.css:265–300`).
    #[default]
    Dark,
}

/// Marker for the shared surface ramp; mode-independent by construction.
#[derive(Clone, Copy, Debug, Default)]
pub struct SurfaceScale;

impl SurfaceScale {
    /// The ramp value for `step`.
    #[must_use]
    pub fn value(self, step: SurfaceStep) -> Oklch {
        step.oklch()
    }
}

/// Mode-resolved semantic colors (`theme.css` light block :8–48 and `.dark`
/// blocks :265–300). Every field is the verbatim source value.
#[derive(Clone, Copy, Debug)]
pub struct ColorTokens {
    /// `--background`.
    pub background: Oklch,
    /// `--foreground-base`, the pure ramp endpoint text color derives from.
    pub foreground_base: Oklch,
    /// `--foreground`: `color-mix(in oklch, base 90%, background)` resolved
    /// per CSS Color 4 powerless-hue rules (see `ArtisanTheme::for_mode`).
    pub foreground: Oklch,
    /// `--foreground-extra`.
    pub foreground_extra: Oklch,
    /// `--highlight`, tint source of hairlines and inset edges.
    pub highlight: Oklch,
    /// `--card`.
    pub card: Oklch,
    /// `--card-foreground`.
    pub card_foreground: Oklch,
    /// `--popover`.
    pub popover: Oklch,
    /// `--popover-foreground`.
    pub popover_foreground: Oklch,
    /// `--primary`.
    pub primary: Oklch,
    /// `--primary-foreground`.
    pub primary_foreground: Oklch,
    /// `--secondary`.
    pub secondary: Oklch,
    /// `--secondary-foreground`.
    pub secondary_foreground: Oklch,
    /// `--muted`.
    pub muted: Oklch,
    /// `--muted-foreground`.
    pub muted_foreground: Oklch,
    /// `--accent`.
    pub accent: Oklch,
    /// `--accent-foreground`.
    pub accent_foreground: Oklch,
    /// `--destructive`.
    pub destructive: Oklch,
    /// `--banner-info`.
    pub banner_info: Oklch,
    /// `--banner-error`.
    pub banner_error: Oklch,
    /// `--banner-warning`.
    pub banner_warning: Oklch,
    /// `--banner-success`.
    pub banner_success: Oklch,
    /// `--favorite`: a star reads as gold, not warning (theme.css:31–35).
    pub favorite: Oklch,
    /// `--unread`: sky reads as new, not alarm (theme.css:36–37).
    pub unread: Oklch,
    /// `--question-from`: purple reads as "your turn" (theme.css:38–39).
    pub question_from: Oklch,
    /// `--question-to`.
    pub question_to: Oklch,
    /// `--border`.
    pub border: Oklch,
    /// `--input`.
    pub input: Oklch,
    /// `--ring`.
    pub ring: Oklch,
    /// `--chart-1` … `--chart-5` (:44–48 / :295–299).
    pub charts: [Oklch; 5],
}

/// The sidebar palette carried by shadcn's own components
/// (`theme.css:253–263` light, :302–311 dark).
#[derive(Clone, Copy, Debug)]
pub struct SidebarColors {
    /// `--sidebar`.
    pub sidebar: Oklch,
    /// `--sidebar-foreground`.
    pub foreground: Oklch,
    /// `--sidebar-primary`.
    pub primary: Oklch,
    /// `--sidebar-primary-foreground`.
    pub primary_foreground: Oklch,
    /// `--sidebar-accent`.
    pub accent: Oklch,
    /// `--sidebar-accent-foreground`.
    pub accent_foreground: Oklch,
    /// `--sidebar-border`.
    pub border: Oklch,
    /// `--sidebar-ring`.
    pub ring: Oklch,
}

/// Font family roles with their known variable-weight ranges
/// (`fonts.css:7–37`; roles per `theme.css:313–315`, `fonts.css:39–42`, and
/// `global.css:48–54`).
#[derive(Clone, Copy, Debug)]
pub struct TypographyTokens {
    /// `--font-sans`: `"Spline Sans"`, weights 300–700.
    pub sans: FontRole,
    /// `--font-mono`: `"Spline Sans Mono"`, weights 300–700.
    pub mono: FontRole,
    /// `--font-heading`: `"Spline Sans"` for headings.
    pub heading: FontRole,
    /// `--font-logo`: `"Spline Sans"`, weights 300–700.
    pub logo: FontRole,
    /// The wordmark face `"Spline Sans"`, weights 300–700.
    pub wordmark: FontRole,
    /// Control text: Tailwind `text-sm`, 14 px (INVENTORY §5.5).
    pub control_text: Pixels,
    /// Composer editor / inputs at the base breakpoint: `text-base`, 16 px
    /// (INVENTORY §5.5).
    pub editor_text_base: Pixels,
    /// Composer editor / inputs at `md:` and wider: `text-sm`, 14 px. The
    /// desktop consumer of this framework uses this token.
    pub editor_text_desktop: Pixels,
    /// Labels, group headings, shortcuts: `text-xs`, 12 px.
    pub label_text: Pixels,
    /// Dialog titles: `text-base font-medium leading-none`, 16 px weight 500
    /// (`ui/dialog/dialog-title.svelte`; INVENTORY §2 row 11).
    pub dialog_title_text: Pixels,
    /// Dialog title font weight (Tailwind `font-medium`).
    pub dialog_title_weight: u16,
}

/// One family name plus its declared variable-weight range.
#[derive(Clone, Copy, Debug)]
pub struct FontRole {
    /// Family name exactly as declared in `@font-face`.
    pub family: &'static str,
    /// Inclusive variable-weight range from the `@font-face` declaration.
    pub weights: WeightRange,
}

impl FontRole {
    /// Binds this role to a GPUI [`Font`] at `weight` for `.font()`/`.font_family()`.
    ///
    /// The family resolves to a vendored face only after
    /// [`crate::fonts::register_bundled_fonts`] runs at startup; before that
    /// GPUI falls back silently through its built-in stack, so the returned
    /// value is always safe to paint with.
    #[must_use]
    pub fn font(&self, weight: FontWeight) -> Font {
        let mut font = gpui::font(self.family);
        font.weight = weight;
        font
    }
}

/// A known inclusive weight range for a variable font.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WeightRange {
    /// Lowest declared weight.
    pub min: u16,
    /// Highest declared weight.
    pub max: u16,
}

impl TypographyTokens {
    /// Display face for headings and titles: `--font-heading`, Spline Sans
    /// (`fonts.css:40`).
    #[must_use]
    pub const fn display(&self) -> &FontRole {
        &self.heading
    }

    /// UI face for body text: `--font-sans`, Spline Sans (`theme.css:313`).
    #[must_use]
    pub const fn body(&self) -> &FontRole {
        &self.sans
    }

    /// Mono face for code and the composer: `--font-mono`, `Spline Sans Mono`
    /// (`theme.css:315`).
    #[must_use]
    pub const fn code(&self) -> &FontRole {
        &self.mono
    }
}

/// Reference conversation-prose metrics shared by the Markdown renderer and
/// the shell/composer surfaces.
///
/// Frozen recipe pinned from the web reference: the Tailwind typography
/// plugin (`@tailwindcss/typography` 0.5.20 DEFAULT theme) for block sizes,
/// margins, and line heights; `prose.css` headings/links/inline-code
/// overrides for per-heading sizes; and the workspace type
/// (`utilities.css` `docs-responsive-surfaces` plus the
/// `.docs-responsive-surfaces .prose` override) for body weight 410 at
/// −0.04 em tracking and heading weight 630 at −0.045 em tracking.
///
/// Shell (`thread_screen`, `native_composer`) and renderer
/// (`conversation_surface`, `inline_code_text`) consumers read these
/// constants instead of re-deriving them so every surface stays on one
/// recipe. Family choice is deliberately not part of this helper: body and
/// headings keep Spline Sans and code keeps Spline Sans Mono pending the
/// open family question, and `editor_text_desktop` is untouched.
///
/// Weights are [`FontWeight`] values (request 410/630 exactly) so the
/// vendored static prose faces match by OS/2 metadata; the WGPU backend
/// does not instantiate variable axes. Tracking is absolute pixels at the
/// size it applies to, because GPUI takes absolute tracking: body −0.04 em
/// at 16 px, headings −0.045 em at each heading size, code keeps normal
/// tracking (`utilities.css:717–719`).
pub struct ProseTypography;

/// Per-heading reference metrics: size, line height, tracking, and
/// block margins for one heading level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProseHeading {
    /// Reference size in px (`text-3xl/2xl/xl/lg` at the 16 px root).
    pub size_px: f32,
    /// Reference line height in px (`leading-tight/snug` factors).
    pub line_px: f32,
    /// −0.045 em tracking resolved at `size_px`.
    pub tracking_px: f32,
    /// Top margin in px (plugin em margins at the overridden size; first
    /// children and `h2/h3/h4 + *` followers take 0 instead).
    pub margin_top_px: f32,
    /// Bottom margin in px; last children take 0 instead.
    pub margin_bottom_px: f32,
}

impl ProseTypography {
    /// Body text: 16 px / 28 px (plugin `1rem`, 28/16).
    pub const BODY_SIZE_PX: f32 = 16.0;
    /// Body line height: 28 px.
    pub const BODY_LINE_PX: f32 = 28.0;
    /// Body weight 410: request exactly so the vendored 410 static matches.
    pub const BODY_WEIGHT: FontWeight = FontWeight(410.0);
    /// Body tracking: −0.04 em at 16 px.
    pub const BODY_TRACKING_PX: f32 = -0.64;
    /// Workspace tracking at any size: −0.04 em resolved at `size_px`, so
    /// 14 px workspace text takes −0.56 px rather than the fixed body
    /// value. Shell and composer consumers use this for non-prose sizes.
    #[must_use]
    pub const fn body_tracking_px(size_px: f32) -> f32 {
        size_px * -0.04
    }
    /// Heading weight 630: request exactly so the vendored 630 static matches.
    pub const HEADING_WEIGHT: FontWeight = FontWeight(630.0);
    /// Strong weight 600 (plugin `strong`), served by the vendored 600 static.
    pub const STRONG_WEIGHT: FontWeight = FontWeight::SEMIBOLD;
    /// Link weight 500 (plugin `a`), served by the vendored 500 static.
    pub const LINK_WEIGHT: FontWeight = FontWeight::MEDIUM;
    /// Inline code size: 14 px (plugin `code` 0.875 em at 16 px). GPUI
    /// highlight runs cannot resize runs, so inline code keeps body size;
    /// only fences render at this size.
    pub const CODE_SIZE_PX: f32 = 14.0;
    /// Fence line height: 24 px (plugin `pre` 24/14).
    pub const CODE_LINE_PX: f32 = 24.0;
    /// Fence body padding: 16 px (`docs-code-snippet-body` `p-4`).
    pub const CODE_PAD_PX: f32 = 16.0;
    /// Fence corner radius: the reference `rounded-3xl`, which resolves
    /// through the workspace ramp (`theme.css:392–399`: base 10 px × 2.2)
    /// to 22 px — not the Tailwind default 24 px. Renderers use the shared
    /// [`RadiusStep::X3l`] token (`RadiusTokens::value`), never a literal.
    /// (No `CODE_RADIUS_PX` constant exists on purpose.)
    /// Paragraph margins: 20 px top and bottom (plugin `p`).
    pub const PARAGRAPH_MARGIN_PX: f32 = 20.0;
    /// Fence margins: 24 px top and bottom (plugin `pre` at 14 px).
    pub const FENCE_MARGIN_PX: f32 = 24.0;
    /// List margins: 20 px top and bottom (plugin `ul`/`ol`).
    pub const LIST_MARGIN_PX: f32 = 20.0;
    /// List indent: 26 px (`padding-inline-start` 1.625 em at 16 px).
    pub const LIST_INDENT_PX: f32 = 26.0;
    /// Item row pitch: 8 px (`li` top/bottom margins, collapsed).
    pub const ITEM_GAP_PX: f32 = 8.0;
    /// Item-scope paragraph margins: 12 px (`> ul > li p`).
    pub const ITEM_PARAGRAPH_MARGIN_PX: f32 = 12.0;
    /// Nested list margins: 12 px (`ul ul` and siblings).
    pub const NESTED_LIST_MARGIN_PX: f32 = 12.0;

    /// Reference metrics for one heading level (1–6; anything else reads
    /// as 6).
    ///
    /// Sizes and line heights come from the `prose.css` overrides
    /// (`text-3xl/2xl/xl/lg` with `leading-tight/snug`); margins come from
    /// the plugin em margins resolved at the overridden size — h1 bottom
    /// is 0.8888889 em at 30 px (26.667 px) and h4–h6 margins are 1.5/0.5
    /// em at the overridden 18 px (27/9 px) — and tracking is −0.045 em at
    /// each size.
    #[must_use]
    pub const fn heading(level: u8) -> ProseHeading {
        match level {
            1 => ProseHeading {
                size_px: 30.0,
                line_px: 37.5,
                tracking_px: -1.35,
                margin_top_px: 0.0,
                margin_bottom_px: 80.0 / 3.0,
            },
            2 => ProseHeading {
                size_px: 24.0,
                line_px: 30.0,
                tracking_px: -1.08,
                margin_top_px: 48.0,
                margin_bottom_px: 24.0,
            },
            3 => ProseHeading {
                size_px: 20.0,
                line_px: 27.5,
                tracking_px: -0.9,
                margin_top_px: 32.0,
                margin_bottom_px: 12.0,
            },
            _ => ProseHeading {
                size_px: 18.0,
                line_px: 24.75,
                tracking_px: -0.81,
                margin_top_px: 27.0,
                margin_bottom_px: 9.0,
            },
        }
    }
}

/// Spacing built on the legacy 4 px base unit (Tailwind's `--spacing`
/// multiplier: `calc(var(--spacing) * n)`; INVENTORY §2 rows 7/9/12, §5.2;
/// PLAN "4 px base spacing unit").
#[derive(Clone, Copy, Debug, Default)]
pub struct SpacingTokens;

impl SpacingTokens {
    /// The base unit: one spacing step is 4 px.
    pub const BASE_PX: f32 = 4.0;

    /// Multiplies the base unit by a Tailwind step count; fractional steps
    /// are used by the audited sources (`py-1.5`, INVENTORY §2 row 9).
    #[must_use]
    pub fn steps(self, count: f32) -> Pixels {
        px(Self::BASE_PX * count)
    }
}

/// The legacy corner ramp. Base `--radius: 0.625rem` = 10 px, multiplied by
/// `0.4 / 0.6 / 0.8 / 1 / 1.4 / 1.8 / 2.2 / 2.6` → **4/6/8/10/14/18/22/26 px**
/// (`theme.css:90,386–399`; INVENTORY §5.2). "Every corner in the app is one
/// of these." Variant names keep the legacy token suffixes (`X2l` ↔ `2xl`).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RadiusStep {
    /// `--radius-xs` — 4 px.
    Xs,
    /// `--radius-sm` — 6 px.
    Sm,
    /// `--radius-md` — 8 px.
    Md,
    /// `--radius-lg` — 10 px.
    Lg,
    /// `--radius-xl` — 14 px.
    Xl,
    /// `--radius-2xl` — 18 px.
    X2l,
    /// `--radius-3xl` — 22 px.
    X3l,
    /// `--radius-4xl` — 26 px.
    X4l,
}

/// Marker holding ramp arithmetic shared across call sites.
#[derive(Clone, Copy, Debug, Default)]
pub struct RadiusTokens;

impl RadiusTokens {
    /// `--radius` itself: 0.625rem at the 16 px root size.
    pub const BASE_PX: f32 = 10.0;

    /// The exact pixel value of a ramp step.
    #[must_use]
    pub const fn value(step: RadiusStep) -> Pixels {
        match step {
            RadiusStep::Xs => px(4.0),
            RadiusStep::Sm => px(6.0),
            RadiusStep::Md => px(8.0),
            RadiusStep::Lg => px(10.0),
            RadiusStep::Xl => px(14.0),
            RadiusStep::X2l => px(18.0),
            RadiusStep::X3l => px(22.0),
            RadiusStep::X4l => px(26.0),
        }
    }

    /// Legacy nested-corner arithmetic:
    /// `--radius-nested: calc(var(--radius-surface) - var(--radius-gap))`
    /// (`utilities.css:1295` area via INVENTORY §5.2; composer sets
    /// `[--radius-surface:var(--radius-2xl)] [--radius-gap:calc(var(--spacing)*2)]`,
    /// `thread-composer.svelte:545,550` → 18 − 8 = 10 px).
    ///
    /// Saturates at zero rather than producing a negative corner.
    #[must_use]
    pub fn nested(surface: Pixels, gap: Pixels) -> Pixels {
        let inner = f32::from(surface) - f32::from(gap);
        px(inner.max(0.0))
    }
}

/// One outer shadow layer, exactly what pinned GPUI can represent:
/// `BoxShadow { color: Hsla, offset: Point<Pixels>, blur_radius: Pixels,
/// spread_radius: Pixels, inset: bool }` (`vendor/gpui-ce/crates/gpui/src/style.rs`).
#[derive(Clone, Copy, Debug)]
pub struct ShadowLayer {
    /// Shadow color including alpha.
    pub color: SrgbComponents,
    /// X offset (positive rightward).
    pub offset_x: Pixels,
    /// Y offset (positive downward).
    pub offset_y: Pixels,
    /// Blur radius.
    pub blur_radius: Pixels,
    /// Spread radius (negative shrinks the shadow).
    pub spread_radius: Pixels,
}

impl ShadowLayer {
    /// Maps onto GPUI's [`gpui::BoxShadow`] field-for-field as an outer
    /// shadow (`inset: false`); no inset semantics are involved on either
    /// side.
    #[must_use]
    pub fn to_box_shadow(self) -> gpui::BoxShadow {
        gpui::BoxShadow {
            color: srgb_to_hsla(self.color),
            offset: gpui::Point {
                x: self.offset_x,
                y: self.offset_y,
            },
            blur_radius: self.blur_radius,
            spread_radius: self.spread_radius,
            inset: false,
        }
    }
}

/// One recorded inset-shadow layer from `--shadow-inset` /
/// `--shadow-inset-artwork` (`theme.css:206–226`).
///
/// These layers deliberately expose *no* GPUI conversion yet: they exist as
/// source-of-truth records so a later renderer seam can honor them honestly
/// instead of faking an outer shadow as an equivalent. (The gpui-ce fork
/// has since grown a `BoxShadow::inset` flag; wiring these records through
/// it is a later packet, not this migration.)
#[derive(Clone, Copy, Debug)]
pub struct InsetShadowLayer {
    /// X offset (all legacy inset sources use 0).
    pub offset_x: Pixels,
    /// Y offset (positive = shadow pulled down inside the edge).
    pub offset_y: Pixels,
    /// Blur radius.
    pub blur_radius: Pixels,
    /// Spread radius.
    pub spread_radius: Pixels,
    /// Layer color including alpha; highlight-derived entries are resolved
    /// per mode at construction (`--highlight` is mode-dependent).
    pub color: SrgbComponents,
}

/// Mode-resolved shadow/elevation values used by the selected workflow.
///
/// - `card` utility stack verbatim (`utilities.css:31–37`), with the
///   highlight-relative layer resolved against this mode's `--highlight`.
/// - `card-lg` utility stack verbatim (`utilities.css:48–54`), same
///   highlight treatment: the code-snippet face.
///
/// - menu/popover elevation: Tailwind `shadow-2xl` on dropdown/context/select
///   contents (`0 25px 50px -12px rgb(0 0 0 / 0.25)`, pinned
///   `tailwindcss/theme.css`; INVENTORY §2 rows 10/21).
/// - both inset stacks as non-renderable records (`theme.css:206–226`).
#[derive(Clone, Copy, Debug)]
pub struct ElevationTokens {
    /// The four outer `@utility card` layers.
    pub card_shadow: [ShadowLayer; 4],
    /// The four outer `@utility card-lg` layers.
    pub card_lg_shadow: [ShadowLayer; 4],
    /// The single floating-menu layer (`shadow-2xl`).
    pub menu_shadow: [ShadowLayer; 1],
    /// `--shadow-inset`, recorded only.
    pub inset: [InsetShadowLayer; 5],
    /// `--shadow-inset-artwork`, recorded only.
    pub inset_artwork: [InsetShadowLayer; 5],
}

/// Control sizes exercised by the selected workflow (INVENTORY §2):
/// button variants h-9/h-6/h-8/h-10 and icon squares of the same edge
/// (row 6), input `h-9` (row 14), select triggers default/sm `h-9`/`h-8`
/// (row 21), switch 32×18.4 / 24×14 px with a 3 px focus ring (row 27),
/// badge `h-5` (row 5), tabs list default `h-9` with 3 px inner padding
/// (row 28), command list `max-h-72` (row 9), card spacing `--spacing(6)`
/// compact `4` (row 7).
#[derive(Clone, Copy, Debug)]
pub struct DensityTokens {
    /// Default control height (`h-9` = 36 px).
    pub control_default: Pixels,
    /// Extra-small control height (`h-6` = 24 px).
    pub control_xs: Pixels,
    /// Small control height (`h-8` = 32 px).
    pub control_sm: Pixels,
    /// Large control height (`h-10` = 40 px).
    pub control_lg: Pixels,
    /// Switch track, default size: 32 × 18.4 px.
    pub switch_default: (Pixels, Pixels),
    /// Switch track, small size: 24 × 14 px.
    pub switch_sm: (Pixels, Pixels),
    /// Badge height (`h-5` = 20 px).
    pub badge_height: Pixels,
    /// Tabs list height (default variant, `h-9`).
    pub tabs_list_height: Pixels,
    /// Tabs list inner padding (`p-[3px]`).
    pub tabs_list_padding: Pixels,
    /// Command palette list max height (`max-h-72` = 288 px).
    pub command_list_max_height: Pixels,
    /// Card vertical padding (`--spacing(6)` = 24 px).
    pub card_padding: Pixels,
    /// Compact card padding (`size sm`: `--spacing(4)` = 16 px).
    pub card_padding_compact: Pixels,
}

/// The shared control-height scale, named after the legacy variant sizes it
/// preserves (button/select/input recipes; INVENTORY §2 rows 6/14/21).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ControlSize {
    /// `h-6` — 24 px.
    Xs,
    /// `h-8` / select `sm` — 32 px.
    Sm,
    /// `h-9`, the default for buttons, inputs, and select triggers.
    Default,
    /// `h-10` — 40 px.
    Lg,
}

impl DensityTokens {
    /// The height of a named control size; identical to reading the explicit
    /// [`DensityTokens`] field of the same name.
    #[must_use]
    pub fn control_height(self, size: ControlSize) -> Pixels {
        match size {
            ControlSize::Xs => self.control_xs,
            ControlSize::Sm => self.control_sm,
            ControlSize::Default => self.control_default,
            ControlSize::Lg => self.control_lg,
        }
    }
}

/// Focus/selection/hover interaction treatments.
///
/// - Focus ring: the app-wide base layer pairs `outline-ring/50` with each
///   control's `focus-visible:ring-[3px] ring-ring/50` (`global.css:25–30`;
///   INVENTORY §2 rows 14/27): 3 px wide, ring color at 50% alpha. Invalid
///   controls swap to destructive at 20% (`aria-invalid:ring-destructive/20`,
///   row 14).
/// - Selection: `::selection` paints `var(--selection)` over
///   `var(--foreground)` (`global.css:63–66`; token at `theme.css:250`).
/// - Hover-pill fill: the named vertical gradient face
///   (`--hover-surface-fill`, `theme.css:244–248`) resolved against this
///   mode's foreground.
#[derive(Clone, Copy, Debug)]
pub struct InteractionTokens {
    /// Focus-ring width (legacy `[3px]`).
    pub focus_ring_width: Pixels,
    /// Ring color at legacy 50% alpha.
    pub focus_ring_color: Oklch,
    /// Invalid-state ring color at legacy 20% alpha.
    pub invalid_ring_color: Oklch,
    /// `::selection` background (`oklch(0.48 0.13 250 / 42%)`).
    pub selection_background: Oklch,
    /// `::selection` text color (this mode's `--foreground`).
    pub selection_foreground: Oklch,
    /// Hover-pill gradient top stop: foreground at 16% alpha.
    pub hover_fill_top: Oklch,
    /// Hover-pill gradient bottom stop: foreground at 7% alpha.
    pub hover_fill_bottom: Oklch,
}

/// The complete Artisan theme, selected explicitly by mode and composed from
/// typed token groups. Values only: nothing here registers globals, observes
/// windows, or renders widgets.
#[derive(Clone, Copy, Debug)]
pub struct ArtisanTheme {
    /// The mode this theme was built for.
    pub mode: ThemeMode,
    /// Shared 41-step neutral ramp (`theme.css:49–89`).
    pub surfaces: SurfaceScale,
    /// Semantic colors for [`Self::mode`].
    pub colors: ColorTokens,
    /// Sidebar palette for [`Self::mode`].
    pub sidebar: SidebarColors,
    /// Family roles, weight ranges, and text-size roles.
    pub typography: TypographyTokens,
    /// The 4 px spacing unit.
    pub spacing: SpacingTokens,
    /// The corner ramp and nested-radius arithmetic.
    pub radius: RadiusTokens,
    /// Card/menu elevation plus recorded inset stacks for this mode.
    pub elevation: ElevationTokens,
    /// Audited control sizes.
    pub density: DensityTokens,
    /// Focus, selection, and hover-fill treatments for this mode.
    pub interaction: InteractionTokens,
}

impl ArtisanTheme {
    /// Builds the theme for an explicit mode.
    ///
    /// `--foreground` resolves the legacy
    /// `color-mix(in oklch, var(--foreground-base) 90%, var(--background))`
    /// (`theme.css:11/:268`): components interpolate linearly in OKLCH and a
    /// zero-chroma endpoint contributes a powerless hue, so the mix keeps the
    /// base hue. Light: `0.9·(0.141 0.005 285.823) + 0.1·(1 0 ·)` →
    /// `oklch(0.2269 0.0045 285.823)`; dark:
    /// `0.9·(0.985 0 ·) + 0.1·(0.141 0.005 285.823)` →
    /// `oklch(0.9006 0.0005 285.823)` (arithmetic pinned by external tests).
    #[must_use]
    pub fn for_mode(mode: ThemeMode) -> Self {
        let colors = match mode {
            ThemeMode::Light => Self::light_colors(),
            ThemeMode::Dark => Self::dark_colors(),
        };
        let sidebar = match mode {
            ThemeMode::Light => Self::light_sidebar(),
            ThemeMode::Dark => Self::dark_sidebar(),
        };
        let highlight = match mode {
            ThemeMode::Light => SurfaceStep::S950.oklch(),
            ThemeMode::Dark => SurfaceStep::S50.oklch(),
        };

        Self {
            mode,
            surfaces: SurfaceScale,
            colors,
            sidebar,
            typography: TypographyTokens {
                sans: FontRole {
                    family: "Spline Sans",
                    weights: WeightRange { min: 300, max: 700 },
                },
                mono: FontRole {
                    family: "Spline Sans Mono",
                    weights: WeightRange { min: 300, max: 700 },
                },
                heading: FontRole {
                    family: "Spline Sans",
                    weights: WeightRange { min: 300, max: 700 },
                },
                logo: FontRole {
                    family: "Spline Sans",
                    weights: WeightRange { min: 300, max: 700 },
                },
                wordmark: FontRole {
                    family: "Spline Sans",
                    weights: WeightRange { min: 300, max: 700 },
                },
                control_text: px(14.0),
                editor_text_base: px(16.0),
                editor_text_desktop: px(14.0),
                label_text: px(12.0),
                dialog_title_text: px(16.0),
                dialog_title_weight: 500,
            },
            spacing: SpacingTokens,
            radius: RadiusTokens,
            elevation: Self::elevation(highlight),
            density: DensityTokens {
                control_default: px(36.0),
                control_xs: px(24.0),
                control_sm: px(32.0),
                control_lg: px(40.0),
                switch_default: (px(32.0), px(18.4)),
                switch_sm: (px(24.0), px(14.0)),
                badge_height: px(20.0),
                tabs_list_height: px(36.0),
                tabs_list_padding: px(3.0),
                command_list_max_height: px(288.0),
                card_padding: px(24.0),
                card_padding_compact: px(16.0),
            },
            interaction: InteractionTokens {
                focus_ring_width: px(3.0),
                focus_ring_color: colors.ring.with_alpha(0.5),
                invalid_ring_color: colors.destructive.with_alpha(0.2),
                selection_background: Oklch::new(0.48, 0.13, 250.0).with_alpha(0.42),
                selection_foreground: colors.foreground,
                hover_fill_top: colors.foreground.with_alpha(0.16),
                hover_fill_bottom: colors.foreground.with_alpha(0.07),
            },
        }
    }

    /// Light-mode semantic tokens, verbatim from `theme.css:8–48`.
    fn light_colors() -> ColorTokens {
        ColorTokens {
            background: SurfaceStep::S0.oklch(),
            foreground_base: SurfaceStep::S950.oklch(),
            foreground: Oklch::new(0.2269, 0.0045, 285.823),
            foreground_extra: SurfaceStep::S950.oklch(),
            highlight: SurfaceStep::S950.oklch(),
            card: SurfaceStep::S0.oklch(),
            card_foreground: SurfaceStep::S950.oklch(),
            popover: SurfaceStep::S0.oklch(),
            popover_foreground: SurfaceStep::S950.oklch(),
            primary: SurfaceStep::S900.oklch(),
            primary_foreground: SurfaceStep::S50.oklch(),
            secondary: SurfaceStep::S100.oklch(),
            secondary_foreground: SurfaceStep::S900.oklch(),
            muted: SurfaceStep::S100.oklch(),
            muted_foreground: SurfaceStep::S500.oklch(),
            accent: SurfaceStep::S100.oklch(),
            accent_foreground: SurfaceStep::S900.oklch(),
            destructive: Oklch::new(0.577, 0.245, 27.325),
            banner_info: Oklch::new(0.623, 0.214, 259.815),
            banner_error: Oklch::new(0.577, 0.245, 27.325),
            banner_warning: Oklch::new(0.681, 0.162, 75.834),
            banner_success: Oklch::new(0.527, 0.154, 150.069),
            favorite: Oklch::new(0.706, 0.153, 78.5),
            unread: Oklch::new(0.685, 0.145, 230.318),
            question_from: Oklch::new(0.558, 0.288, 302.321),
            question_to: Oklch::new(0.714, 0.203, 305.504),
            border: SurfaceStep::S200.oklch(),
            input: SurfaceStep::S200.oklch(),
            ring: SurfaceStep::S400.oklch(),
            charts: [
                SurfaceStep::S300.oklch(),
                SurfaceStep::S500.oklch(),
                SurfaceStep::S600.oklch(),
                SurfaceStep::S700.oklch(),
                SurfaceStep::S800.oklch(),
            ],
        }
    }

    /// Dark-mode semantic tokens, verbatim from `theme.css:265–300`.
    fn dark_colors() -> ColorTokens {
        ColorTokens {
            background: SurfaceStep::S950.oklch(),
            foreground_base: SurfaceStep::S50.oklch(),
            foreground: Oklch::new(0.9006, 0.0005, 285.823),
            foreground_extra: SurfaceStep::S50.oklch(),
            highlight: SurfaceStep::S50.oklch(),
            card: SurfaceStep::S900.oklch(),
            card_foreground: SurfaceStep::S50.oklch(),
            popover: SurfaceStep::S900.oklch(),
            popover_foreground: SurfaceStep::S50.oklch(),
            primary: SurfaceStep::S200.oklch(),
            primary_foreground: SurfaceStep::S900.oklch(),
            secondary: SurfaceStep::S800.oklch(),
            secondary_foreground: SurfaceStep::S50.oklch(),
            muted: SurfaceStep::S800.oklch(),
            muted_foreground: SurfaceStep::S400.oklch(),
            accent: SurfaceStep::S800.oklch(),
            accent_foreground: SurfaceStep::S50.oklch(),
            destructive: Oklch::new(0.704, 0.191, 22.216),
            banner_info: Oklch::new(0.707, 0.165, 254.624),
            banner_error: Oklch::new(0.704, 0.191, 22.216),
            banner_warning: Oklch::new(0.795, 0.184, 86.047),
            banner_success: Oklch::new(0.723, 0.219, 149.579),
            favorite: Oklch::new(0.823, 0.158, 82.5),
            unread: Oklch::new(0.828, 0.111, 230.318),
            question_from: Oklch::new(0.627, 0.265, 303.9),
            question_to: Oklch::new(0.827, 0.119, 306.383),
            border: Oklch::new(1.0, 0.0, 0.0).with_alpha(0.10),
            input: Oklch::new(1.0, 0.0, 0.0).with_alpha(0.15),
            ring: SurfaceStep::S500.oklch(),
            charts: [
                SurfaceStep::S300.oklch(),
                SurfaceStep::S500.oklch(),
                SurfaceStep::S600.oklch(),
                SurfaceStep::S700.oklch(),
                SurfaceStep::S800.oklch(),
            ],
        }
    }

    /// Light sidebar palette (`theme.css:253–263`). Every neutral value is
    /// character-identical to a ramp entry, so the ramp is referenced
    /// directly instead of duplicating literals.
    fn light_sidebar() -> SidebarColors {
        SidebarColors {
            sidebar: SurfaceStep::S50.oklch(),
            foreground: SurfaceStep::S950.oklch(),
            primary: SurfaceStep::S900.oklch(),
            primary_foreground: SurfaceStep::S50.oklch(),
            accent: SurfaceStep::S100.oklch(),
            accent_foreground: SurfaceStep::S900.oklch(),
            border: SurfaceStep::S200.oklch(),
            ring: SurfaceStep::S400.oklch(),
        }
    }

    /// Dark sidebar palette (`theme.css:302–311`). Only `--sidebar-primary`
    /// (an off-ramp blue literal) and the alpha hairline stay direct.
    fn dark_sidebar() -> SidebarColors {
        SidebarColors {
            sidebar: SurfaceStep::S900.oklch(),
            foreground: SurfaceStep::S50.oklch(),
            primary: Oklch::new(0.488, 0.243, 264.376),
            primary_foreground: SurfaceStep::S50.oklch(),
            accent: SurfaceStep::S800.oklch(),
            accent_foreground: SurfaceStep::S50.oklch(),
            border: Oklch::new(1.0, 0.0, 0.0).with_alpha(0.10),
            ring: SurfaceStep::S500.oklch(),
        }
    }

    /// Elevation for one mode: the `card` utility stack with its
    /// highlight-derived hairline resolved, the floating-menu `shadow-2xl`,
    /// and both inset stacks as recorded-only layers.
    fn elevation(highlight: Oklch) -> ElevationTokens {
        let white = SrgbComponents {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        };
        let black = SrgbComponents {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        };
        let layer = |color: SrgbComponents, ox: f32, oy: f32, blur: f32, spread: f32| ShadowLayer {
            color,
            offset_x: px(ox),
            offset_y: px(oy),
            blur_radius: px(blur),
            spread_radius: px(spread),
        };
        let inset = |color: SrgbComponents, oy: f32, blur: f32, spread: f32| InsetShadowLayer {
            offset_x: px(0.0),
            offset_y: px(oy),
            blur_radius: px(blur),
            spread_radius: px(spread),
            color,
        };
        // `@utility card`, utilities.css:31–37.
        let card_shadow = [
            layer(with_alpha(white, 0.08), 0.0, -0.5, 0.0, 0.0),
            layer(with_alpha(black, 0.06), 0.0, 4.0, 8.0, 0.0),
            layer(oklch_srgb(highlight.with_alpha(0.08)), 0.0, 0.0, 0.0, 0.5),
            layer(black, 0.0, 1.0, 6.0, -4.0),
        ];
        // `@utility card-lg`, utilities.css:48–54: the code-snippet face.
        let card_lg_shadow = [
            layer(with_alpha(white, 0.12), 0.0, -0.5, 0.0, 0.0),
            layer(with_alpha(black, 0.32), 0.0, 10.0, 24.0, -12.0),
            layer(with_alpha(black, 0.2), 0.0, 4.0, 12.0, -8.0),
            layer(oklch_srgb(highlight.with_alpha(0.12)), 0.0, 0.0, 0.0, 0.5),
        ];
        // Tailwind `--shadow-2xl` on dropdown/context/select content.
        let menu_shadow = [layer(with_alpha(black, 0.25), 0.0, 25.0, 50.0, -12.0)];
        // `--shadow-inset`, theme.css:207–211.
        let inset_stack = [
            inset(with_alpha(black, 0.06), 1.0, 1.0, -0.5),
            inset(with_alpha(black, 0.06), 3.0, 3.0, -1.5),
            inset(with_alpha(black, 0.06), 6.0, 6.0, -3.0),
            inset(with_alpha(white, 0.08), -0.5, 0.0, 0.0),
            inset(oklch_srgb(highlight.with_alpha(0.08)), 0.0, 0.0, 0.5),
        ];
        // `--shadow-inset-artwork`, theme.css:222–226.
        let artwork_stack = [
            inset(with_alpha(black, 0.30), 1.0, 1.0, -0.5),
            inset(with_alpha(black, 0.26), 3.0, 3.0, -1.5),
            inset(with_alpha(black, 0.22), 6.0, 6.0, -3.0),
            inset(with_alpha(white, 0.50), -0.5, 0.0, 0.0),
            inset(oklch_srgb(highlight.with_alpha(0.35)), 0.0, 0.0, 0.5),
        ];
        ElevationTokens {
            card_shadow,
            card_lg_shadow,
            menu_shadow,
            inset: inset_stack,
            inset_artwork: artwork_stack,
        }
    }
}

/// Copies an [`Oklch`] value into sRGB component storage for shadow layers.
fn oklch_srgb(color: Oklch) -> SrgbComponents {
    color.to_srgb()
}

/// One sRGB base color at a new alpha (legacy `rgb(.. / A)` notation).
fn with_alpha(mut base: SrgbComponents, a: f32) -> SrgbComponents {
    base.a = a;
    base
}

/// Shared tokens for the native desktop workspace.
///
/// The desktop shell deliberately has a smaller, quieter palette than the
/// legacy application surfaces. Keeping it here makes the shell and its
/// controls agree on the neutral-dark art direction without introducing
/// frontend-local color constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DesktopTheme {
    /// Native titlebar and outer chrome background (`#0a0a0a`).
    pub chrome: Hsla,
    /// Main workspace background (`#0a0a0a`).
    pub workspace: Hsla,
    /// Sidebar and composer surface (`#111111`).
    pub sidebar: Hsla,
    /// Hairline rules (`#292929`).
    pub line: Hsla,
    /// Primary text (`#ededed`).
    pub foreground: Hsla,
    /// Secondary text (`#a1a1a1`).
    pub secondary: Hsla,
    /// Selected row background (`#242424`).
    pub selected: Hsla,
    /// Primary action background (`#ededed`).
    pub primary_action: Hsla,
    /// Primary action foreground (`#0a0a0a`).
    pub primary_action_foreground: Hsla,
    /// Search and composer input surface (`#171717`).
    pub field: Hsla,
    /// Input border (`#333333`).
    pub field_line: Hsla,
    /// Search result surface border (`#383838`).
    pub popover_line: Hsla,
    /// Keyboard shortcut capsule border (`#3a3a3a`).
    pub shortcut_line: Hsla,
    /// Slightly brighter crosshair stroke than the surrounding rule.
    pub crosshair: Hsla,
}

impl DesktopTheme {
    /// The accepted neutral-dark native workspace palette.
    #[must_use]
    pub fn neutral_dark() -> Self {
        Self {
            chrome: desktop_color(0x000a_0a0a),
            workspace: desktop_color(0x000a_0a0a),
            sidebar: desktop_color(0x0011_1111),
            line: desktop_color(0x0029_2929),
            foreground: desktop_color(0x00ed_eded),
            secondary: desktop_color(0x00a1_a1a1),
            selected: desktop_color(0x0024_2424),
            primary_action: desktop_color(0x00ed_eded),
            primary_action_foreground: desktop_color(0x000a_0a0a),
            field: desktop_color(0x0017_1717),
            field_line: desktop_color(0x0033_3333),
            popover_line: desktop_color(0x0038_3838),
            shortcut_line: desktop_color(0x003a_3a3a),
            crosshair: desktop_color(0x0066_6666),
        }
    }
}

impl Default for DesktopTheme {
    fn default() -> Self {
        Self::neutral_dark()
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "each sRGB channel is a masked byte, so the i32-to-f32 conversion is exact"
)]
fn desktop_color(hex: u32) -> Hsla {
    srgb_to_hsla(SrgbComponents {
        r: ((hex >> 16) & 0xff) as f32 / 255.0,
        g: ((hex >> 8) & 0xff) as f32 / 255.0,
        b: (hex & 0xff) as f32 / 255.0,
        a: 1.0,
    })
}
