//! Typed application theme, resolved once per mode. OKLCH source colors retain cached GPUI paint values.

use std::sync::OnceLock;

use gpui::{FontWeight, Hsla, Pixels, px};

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

/// An OKLCH source color with a cached GPUI paint conversion.
#[derive(Clone, Copy, Debug)]
pub struct Oklch {
    /// Perceptual lightness, `0..=1`.
    pub l: f32,
    /// Chroma, `>= 0`.
    pub c: f32,
    /// Hue angle in degrees.
    pub h: f32,
    /// Alpha, `0..=1`.
    pub a: f32,
    paint: Option<([f32; 4], Hsla)>,
}

impl PartialEq for Oklch {
    fn eq(&self, other: &Self) -> bool {
        self.l == other.l && self.c == other.c && self.h == other.h && self.a == other.a
    }
}

impl Oklch {
    /// Constructs an opaque OKLCH color.
    #[must_use]
    pub const fn new(l: f32, c: f32, h: f32) -> Self {
        Self {
            l,
            c,
            h,
            a: 1.0,
            paint: None,
        }
    }

    /// The same color at an explicit alpha (`oklch(.. / A)`).
    #[must_use]
    pub const fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        if let Some((mut source, mut paint)) = self.paint {
            source[3] = a;
            paint.alpha = a;
            self.paint = Some((source, paint));
        }
        self
    }

    /// Convert through the shared color library, clamping only display channels.
    #[must_use]
    pub fn to_srgb(self) -> SrgbComponents {
        let [r, g, b, a] = color::AlphaColor::<color::Oklch>::new([self.l, self.c, self.h, self.a])
            .convert::<color::Srgb>()
            .components;
        SrgbComponents {
            r: r.clamp(0.0, 1.0),
            g: g.clamp(0.0, 1.0),
            b: b.clamp(0.0, 1.0),
            a,
        }
    }

    /// Builds the GPUI paint color for this source value.
    ///
    /// The clamped sRGB triple is converted with the standard sRGB→HSL math;
    /// that mapping is bijective for in-gamut colors, so GPUI's own paint-time
    /// HSL→RGB reproduces the pinned sRGB within float precision.
    #[must_use]
    pub fn to_paint(self) -> Hsla {
        match self.paint {
            Some((source, paint)) if source == [self.l, self.c, self.h, self.a] => paint,
            _ => srgb_to_hsla(self.to_srgb()),
        }
    }

    fn resolved(mut self) -> Self {
        self.paint = Some(([self.l, self.c, self.h, self.a], self.to_paint()));
        self
    }
}

/// GPUI owns its sRGB-to-paint conversion.
fn srgb_to_hsla(c: SrgbComponents) -> Hsla {
    gpui::rgb_to_hsla(gpui::Rgba::new(c.r, c.g, c.b, c.a))
}

/// Shared neutral surface ramp from white (S0) to black (S1000).
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

    /// The OKLCH source value for this surface step.
    #[must_use]
    pub fn oklch(self) -> Oklch {
        static COLORS: OnceLock<[Oklch; 41]> = OnceLock::new();
        COLORS.get_or_init(|| Self::ALL.map(|step| step.source().resolved()))[self as usize]
    }

    const fn source(self) -> Oklch {
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

/// Light or dark presentation; dark is the default.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThemeMode {
    /// Light presentation.
    Light,
    /// Dark presentation and the application default.
    #[default]
    Dark,
}

/// Semantic colors resolved for the selected mode.
#[derive(Clone, Copy, Debug, PartialEq)]
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
    /// `--favorite`: a star reads as gold, not warning.
    pub favorite: Oklch,
    /// `--unread`: sky reads as new, not alarm.
    pub unread: Oklch,
    /// `--question-from`: purple reads as "your turn".
    pub question_from: Oklch,
    /// `--question-to`.
    pub question_to: Oklch,
    /// A skill named in the composer: purple-pink, apart from links.
    pub mention_skill: Oklch,
    /// A connector (MCP server) named in the composer: link blue.
    pub mention_connector: Oklch,
    /// A command named in the composer: amber, read as an instruction.
    pub mention_command: Oklch,
    /// `--border`.
    pub border: Oklch,
    /// `--input`.
    pub input: Oklch,
    /// `--ring`.
    pub ring: Oklch,
    /// `--chart-1` … `--chart-5` (:44–48 / :295–299).
    pub charts: [Oklch; 5],
}

/// Font families and text sizes shared by application components.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypographyTokens {
    /// `--font-sans`: `"Spline Sans"`, weights 300–700.
    pub sans: FontRole,
    /// `--font-mono`: `"Spline Sans Mono"`, weights 300–700.
    pub mono: FontRole,
    /// `--font-heading`: `"Spline Sans"` for headings.
    pub heading: FontRole,
    /// Control text, 14 px.
    pub control_text: Pixels,
    /// Editor text, 14 px.
    pub editor_text_desktop: Pixels,
    /// Labels, group headings, shortcuts: `text-xs`, 12 px.
    pub label_text: Pixels,
}

/// A bundled font family.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontRole {
    /// Bundled family name.
    pub family: &'static str,
}

/// Shared prose sizes, weights, tracking and block spacing.
/// Tracking values are absolute pixels at the corresponding font size.
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
    /// The app's tracking: −0.04 em. The window root sets it once with
    /// GPUI's `tracking`, so every sans text inherits it at its own size;
    /// monospaced code opts out with `tracking(0.0)`.
    pub const TRACKING_EM: f32 = -0.04;
    /// Workspace tracking at any size: [`Self::TRACKING_EM`] resolved at
    /// `size_px`, so 14 px workspace text takes −0.56 px rather than the
    /// fixed body value.
    #[must_use]
    pub const fn body_tracking_px(size_px: f32) -> f32 {
        size_px * Self::TRACKING_EM
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
    /// Fence corner radius: the composer card's 18 px, which is the
    /// workspace ramp's `2xl` step. Renderers use the shared
    /// [`RadiusStep::X2l`] token (`RadiusTokens::value`), never a literal.
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
    /// Table text size: 14 px (plugin `table` 0.875 em at 16 px).
    pub const TABLE_SIZE_PX: f32 = 14.0;
    /// Table line height: 24 px (plugin `table` 24/14).
    pub const TABLE_LINE_PX: f32 = 24.0;
    /// Table margins: 28 px top and bottom (plugin `table` 2 em at 14 px).
    pub const TABLE_MARGIN_PX: f32 = 28.0;
    /// Table cell padding: 8 px (plugin `th`/`td` 0.571 em at 14 px). The
    /// outer edges of the first and last column take none.
    pub const TABLE_CELL_PAD_PX: f32 = 8.0;

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

/// Spacing uses a 4 px base unit.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SpacingTokens;

impl SpacingTokens {
    /// The base unit: one spacing step is 4 px.
    pub const BASE_PX: f32 = 4.0;

    /// Multiplies the base unit by a possibly fractional step count.
    #[must_use]
    pub fn steps(self, count: f32) -> Pixels {
        px(Self::BASE_PX * count)
    }
}

/// Shared corner radii: 4, 6, 8, 10, 14, 18, 22 and 26 px.
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
#[derive(Clone, Copy, Debug, Default, PartialEq)]
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

    /// Subtracts the inset gap from an outer radius, clamping at zero.
    #[must_use]
    pub fn nested(surface: Pixels, gap: Pixels) -> Pixels {
        let inner = f32::from(surface) - f32::from(gap);
        px(inner.max(0.0))
    }
}

/// One outer shadow layer, exactly what pinned GPUI can represent:
/// `BoxShadow { color: Hsla, offset: Point<Pixels>, blur_radius: Pixels,
/// spread_radius: Pixels, inset: bool }` (`artisanstreet/gpui-ce` crates/gpui/src/style.rs).
#[derive(Clone, Copy, Debug, PartialEq)]
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

/// Theme-resolved card and floating-menu shadows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevationTokens {
    /// The four outer `@utility card` layers.
    pub card_shadow: [ShadowLayer; 4],
    /// The four outer `@utility card-lg` layers.
    pub card_lg_shadow: [ShadowLayer; 4],
    /// The single floating-menu layer (`shadow-2xl`).
    pub menu_shadow: [ShadowLayer; 1],
}

/// Shared component dimensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensityTokens {
    /// Standard segmented control height (`h-9` = 36 px).
    pub control_default: Pixels,
    /// Scrollable select menu viewport, matching the original command list.
    pub command_list_max_height: Pixels,
    /// Small control height (`h-8` = 32 px).
    pub control_sm: Pixels,
    /// Switch track, default size: 32 × 18.4 px.
    pub switch_default: (Pixels, Pixels),
    /// Switch track, small size: 24 × 14 px.
    pub switch_sm: (Pixels, Pixels),
    /// Badge height (`h-5` = 20 px).
    pub badge_height: Pixels,
    /// Compact card padding (`size sm`: `--spacing(4)` = 16 px).
    pub card_padding_compact: Pixels,
}

/// Shared focus rings, text selection and hover gradient colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteractionTokens {
    /// Focus-ring width, 3 px.
    pub focus_ring_width: Pixels,
    /// Focus-ring color at 50% alpha.
    pub focus_ring_color: Oklch,
    /// Invalid-state ring color at 20% alpha.
    pub invalid_ring_color: Oklch,
    /// Text-selection background: `#1967D2` (`oklch(0.5318 0.1807 258.26)`).
    pub selection_background: Oklch,
    /// Hover-pill gradient top stop: foreground at 16% alpha.
    pub hover_fill_top: Oklch,
    /// Hover-pill gradient bottom stop: foreground at 7% alpha.
    pub hover_fill_bottom: Oklch,
}

/// The complete Artisan theme, selected explicitly by mode and composed from
/// typed token groups. Values only: nothing here registers globals, observes
/// windows, or renders widgets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtisanTheme {
    /// The mode this theme was built for.
    pub mode: ThemeMode,
    /// Native workspace roles resolved with the selected theme.
    pub workspace: WorkspaceColors,
    /// Semantic colors for [`Self::mode`].
    pub colors: ColorTokens,
    /// Font families and text sizes.
    pub typography: TypographyTokens,
    /// The 4 px spacing unit.
    pub spacing: SpacingTokens,
    /// The corner ramp and nested-radius arithmetic.
    pub radius: RadiusTokens,
    /// Card and menu shadows for this mode.
    pub elevation: ElevationTokens,
    /// Shared control sizes.
    pub density: DensityTokens,
    /// Focus, selection, and hover-fill treatments for this mode.
    pub interaction: InteractionTokens,
}

impl ArtisanTheme {
    /// Returns the cached theme for this mode.
    #[must_use]
    pub fn for_mode(mode: ThemeMode) -> Self {
        static LIGHT: OnceLock<ArtisanTheme> = OnceLock::new();
        static DARK: OnceLock<ArtisanTheme> = OnceLock::new();
        *match mode {
            ThemeMode::Light => &LIGHT,
            ThemeMode::Dark => &DARK,
        }
        .get_or_init(|| Self::build(mode))
    }

    fn build(mode: ThemeMode) -> Self {
        let colors = match mode {
            ThemeMode::Light => Self::light_colors(),
            ThemeMode::Dark => Self::dark_colors(),
        };
        let highlight = match mode {
            ThemeMode::Light => SurfaceStep::S950.oklch(),
            ThemeMode::Dark => SurfaceStep::S50.oklch(),
        };

        let mut theme = Self {
            mode,
            workspace: WorkspaceColors::for_mode(mode),
            colors,
            typography: TypographyTokens {
                sans: FontRole {
                    family: "Spline Sans",
                },
                mono: FontRole {
                    family: "Spline Sans Mono",
                },
                heading: FontRole {
                    family: "Spline Sans",
                },
                control_text: px(14.0),
                editor_text_desktop: px(14.0),
                label_text: px(12.0),
            },
            spacing: SpacingTokens,
            radius: RadiusTokens,
            elevation: Self::elevation(highlight),
            density: DensityTokens {
                control_default: px(36.0),
                command_list_max_height: px(288.0),
                control_sm: px(32.0),
                switch_default: (px(32.0), px(18.4)),
                switch_sm: (px(24.0), px(14.0)),
                badge_height: px(20.0),
                card_padding_compact: px(16.0),
            },
            interaction: InteractionTokens {
                focus_ring_width: px(3.0),
                focus_ring_color: colors.ring.with_alpha(0.5),
                invalid_ring_color: colors.destructive.with_alpha(0.2),
                selection_background: Oklch::new(0.5318, 0.1807, 258.26).with_alpha(match mode {
                    ThemeMode::Light => 0.3,
                    ThemeMode::Dark => 1.0,
                }),
                hover_fill_top: colors.foreground.with_alpha(0.16),
                hover_fill_bottom: colors.foreground.with_alpha(0.07),
            },
        };
        theme.colors.background = theme.colors.background.resolved();
        theme.colors.foreground_base = theme.colors.foreground_base.resolved();
        theme.colors.foreground = theme.colors.foreground.resolved();
        theme.colors.foreground_extra = theme.colors.foreground_extra.resolved();
        theme.colors.highlight = theme.colors.highlight.resolved();
        theme.colors.card = theme.colors.card.resolved();
        theme.colors.card_foreground = theme.colors.card_foreground.resolved();
        theme.colors.popover = theme.colors.popover.resolved();
        theme.colors.popover_foreground = theme.colors.popover_foreground.resolved();
        theme.colors.primary = theme.colors.primary.resolved();
        theme.colors.primary_foreground = theme.colors.primary_foreground.resolved();
        theme.colors.secondary = theme.colors.secondary.resolved();
        theme.colors.secondary_foreground = theme.colors.secondary_foreground.resolved();
        theme.colors.muted = theme.colors.muted.resolved();
        theme.colors.muted_foreground = theme.colors.muted_foreground.resolved();
        theme.colors.accent = theme.colors.accent.resolved();
        theme.colors.accent_foreground = theme.colors.accent_foreground.resolved();
        theme.colors.destructive = theme.colors.destructive.resolved();
        theme.colors.banner_info = theme.colors.banner_info.resolved();
        theme.colors.banner_error = theme.colors.banner_error.resolved();
        theme.colors.banner_warning = theme.colors.banner_warning.resolved();
        theme.colors.banner_success = theme.colors.banner_success.resolved();
        theme.colors.favorite = theme.colors.favorite.resolved();
        theme.colors.unread = theme.colors.unread.resolved();
        theme.colors.question_from = theme.colors.question_from.resolved();
        theme.colors.question_to = theme.colors.question_to.resolved();
        theme.colors.mention_skill = theme.colors.mention_skill.resolved();
        theme.colors.mention_connector = theme.colors.mention_connector.resolved();
        theme.colors.mention_command = theme.colors.mention_command.resolved();
        theme.colors.border = theme.colors.border.resolved();
        theme.colors.input = theme.colors.input.resolved();
        theme.colors.ring = theme.colors.ring.resolved();
        theme.interaction.focus_ring_color = theme.interaction.focus_ring_color.resolved();
        theme.interaction.invalid_ring_color = theme.interaction.invalid_ring_color.resolved();
        theme.interaction.selection_background = theme.interaction.selection_background.resolved();
        theme.interaction.hover_fill_top = theme.interaction.hover_fill_top.resolved();
        theme.interaction.hover_fill_bottom = theme.interaction.hover_fill_bottom.resolved();
        theme.colors.charts = theme.colors.charts.map(Oklch::resolved);
        theme
    }

    /// Light-mode semantic colors.
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
            mention_skill: Oklch::new(0.591, 0.233, 328.0),
            mention_connector: Oklch::new(0.623, 0.214, 259.815),
            mention_command: Oklch::new(0.666, 0.179, 58.318),
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

    /// Dark-mode semantic colors.
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
            mention_skill: Oklch::new(0.746, 0.17, 328.0),
            mention_connector: Oklch::new(0.707, 0.165, 254.624),
            mention_command: Oklch::new(0.828, 0.152, 74.0),
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
        // Compact card shadows.
        let card_shadow = [
            layer(with_alpha(white, 0.08), 0.0, -0.5, 0.0, 0.0),
            layer(with_alpha(black, 0.06), 0.0, 4.0, 8.0, 0.0),
            layer(oklch_srgb(highlight.with_alpha(0.08)), 0.0, 0.0, 0.0, 0.5),
            layer(black, 0.0, 1.0, 6.0, -4.0),
        ];
        // Large card shadows, used by code snippets.
        let card_lg_shadow = [
            layer(with_alpha(white, 0.12), 0.0, -0.5, 0.0, 0.0),
            layer(with_alpha(black, 0.32), 0.0, 10.0, 24.0, -12.0),
            layer(with_alpha(black, 0.2), 0.0, 4.0, 12.0, -8.0),
            layer(oklch_srgb(highlight.with_alpha(0.12)), 0.0, 0.0, 0.0, 0.5),
        ];
        // Tailwind `--shadow-2xl` on dropdown/context/select content.
        let menu_shadow = [layer(with_alpha(black, 0.25), 0.0, 25.0, 50.0, -12.0)];
        ElevationTokens {
            card_shadow,
            card_lg_shadow,
            menu_shadow,
        }
    }
}

/// Copies an [`Oklch`] value into sRGB component storage for shadow layers.
fn oklch_srgb(color: Oklch) -> SrgbComponents {
    color.to_srgb()
}

/// An sRGB color with a new alpha.
fn with_alpha(mut base: SrgbComponents, a: f32) -> SrgbComponents {
    base.a = a;
    base
}

/// Shared colors for native workspace chrome and controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkspaceColors {
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
}

impl WorkspaceColors {
    /// The accepted neutral-dark native workspace palette.
    #[must_use]
    fn for_mode(mode: ThemeMode) -> Self {
        if mode == ThemeMode::Light {
            let colors = ArtisanTheme::light_colors();
            return Self {
                chrome: colors.background.to_paint(),
                workspace: colors.background.to_paint(),
                sidebar: colors.card.to_paint(),
                line: colors.border.to_paint(),
                foreground: colors.foreground.to_paint(),
                secondary: colors.muted_foreground.to_paint(),
                selected: colors.accent.to_paint(),
                primary_action: colors.primary.to_paint(),
                primary_action_foreground: colors.primary_foreground.to_paint(),
                field: colors.muted.to_paint(),
                field_line: colors.input.to_paint(),
                popover_line: colors.border.to_paint(),
                shortcut_line: colors.border.to_paint(),
            };
        }
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
        }
    }
}

impl Default for ArtisanTheme {
    fn default() -> Self {
        Self::for_mode(ThemeMode::Dark)
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

#[cfg(test)]
mod reform_tests {
    use super::*;
    #[test]
    fn semantic_equality_ignores_paint_cache_and_tracks_public_changes() {
        let source = Oklch::new(0.5, 0.03, 240.0);
        let cached = source.resolved();
        assert_eq!(source, cached);
        assert_eq!(source.with_alpha(0.25), cached.with_alpha(0.25));
        let theme = ArtisanTheme::default();
        let mut uncached = theme;
        let color = theme.colors.foreground;
        uncached.colors.foreground = Oklch::new(color.l, color.c, color.h).with_alpha(color.a);
        assert_eq!(theme, uncached);
        assert_eq!(
            theme.colors.foreground.to_paint(),
            uncached.colors.foreground.to_paint()
        );
        uncached.colors.foreground.a = 0.25;
        assert_ne!(theme, uncached);
        assert_ne!(
            theme.colors.foreground.to_paint(),
            uncached.colors.foreground.to_paint()
        );
        uncached = theme;
        uncached.colors.foreground.l = 0.2;
        assert_ne!(theme, uncached);
        assert_eq!(
            uncached.colors.foreground.to_paint(),
            Oklch::new(0.2, color.c, color.h)
                .with_alpha(color.a)
                .to_paint()
        );
    }
    #[test]
    fn cached_paint_tracks_public_color_changes_and_alpha() {
        let color = Oklch::new(0.5, 0.0, 0.0).resolved();
        assert_eq!(color.with_alpha(0.25).to_paint().alpha, 0.25);
        let mut changed = color;
        changed.l = 0.7;
        assert_eq!(changed.to_paint(), Oklch::new(0.7, 0.0, 0.0).to_paint());
        assert_ne!(changed.to_paint(), color.to_paint());
    }
}
