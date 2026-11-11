use std::sync::OnceLock;

use aqua_text::FontWeight;

pub type Pixels = f32;

const fn px(value: f32) -> Pixels {
    value
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SrgbComponents {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}
#[derive(Clone, Copy, Debug)]
pub struct Oklch {
    pub(crate) l: f32,
    pub(crate) c: f32,
    pub(crate) h: f32,
    pub(crate) a: f32,
    paint: Option<([f32; 4], palette::Hsla)>,
}

impl PartialEq for Oklch {
    fn eq(&self, other: &Self) -> bool {
        self.l == other.l && self.c == other.c && self.h == other.h && self.a == other.a
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Opacity {
    Disabled,
    Glass,
    GlassStrong,
    Clear,
    GlassFaceQuiet,
    GlassFaceQuietFar,
    GlassFaceStrong,
    GlassFaceStrongFar,
    GlassBase,
    GlassPane,
    GlassPaneLit,
    SheenFaint,
    SheenSoft,
    Sheen,
    SheenStrong,
    RimLight,
    RimInner,
    RimShade,
    Hairline,
    HairlineFaint,
    HoverTop,
    HoverBottom,
    HoverRing,
    DestructiveHoverStrong,
    DestructiveHoverSoft,
    GlassBorder,
    ToneTint,
    ToneIcon,
    Rule,
    EngineLean,
    EngineEdge,
    MeterTrack,
    ProgressTrack,
    Unavailable,
    Backdrop,
    SettingsBackdrop,
    Shade,
    BackdropDense,
    MediaEdge,
    FocusRing,
    SwatchEdge,
    TerminalDim,
    MentionGlyph,
    ChartRule,
    ChartSelection,
    AnnotationRule,
    EventRule,
    AnnotationBand,
    AreaStacked,
    Area,
    ReferenceLine,
    AreaQuiet,
    Overlap,
    BarSolo,
    BoxFill,
    Outlier,
    LegendHidden,
    LabelBacking,
    Dimmed,
    TreemapLeaf,
    TreemapLeafHover,
    TreemapGroup,
    TreemapEdge,
    TreemapHeader,
    TreemapFocusLeaf,
    TreemapFocusLeafHover,
    TreemapFocusGroup,
    TreemapFocusEdge,
    TreemapFocusHeader,
    TreemapFocusSwatch,
    NodeDimmed,
    Flow,
    FlowLit,
    FlowDimmed,
    LabelDimmed,
    Span,
    SpanHover,
    BadgeActive,
    BadgeTint,
    AnnotationEdge,
    NoteHover,
    CalloutTint,
}

impl Opacity {
    #[must_use]
    pub const fn alpha(self) -> f32 {
        match self {
            Self::Disabled => 0.5,
            Self::Glass => 0.72,
            Self::GlassStrong => 0.86,
            Self::Clear => 0.0,
            Self::GlassFaceQuiet => 0.20,
            Self::GlassFaceQuietFar => 0.14,
            Self::GlassFaceStrong => 0.22,
            Self::GlassFaceStrongFar => 0.16,
            Self::GlassBase => 0.05,
            Self::GlassPane => 0.62,
            Self::GlassPaneLit => 0.30,
            Self::SheenFaint => 0.035,
            Self::SheenSoft => 0.04,
            Self::Sheen => 0.05,
            Self::SheenStrong => 0.08,
            Self::RimLight => 0.14,
            Self::RimInner => 0.10,
            Self::RimShade => 0.22,
            Self::Hairline => 0.08,
            Self::HairlineFaint => 0.04,
            Self::HoverTop => 0.16,
            Self::HoverBottom => 0.07,
            Self::HoverRing => 0.13,
            Self::DestructiveHoverStrong => 0.28,
            Self::DestructiveHoverSoft => 0.14,
            Self::GlassBorder => 0.10,
            Self::ToneTint => 0.10,
            Self::ToneIcon => 0.8,
            Self::Rule => 0.10,
            Self::EngineLean => 0.14,
            Self::EngineEdge => 0.35,
            Self::MeterTrack => 0.11,
            Self::ProgressTrack => 0.18,
            Self::Unavailable => 0.35,
            Self::Backdrop => 0.9,
            Self::SettingsBackdrop => 0.24,
            Self::Shade => 0.45,
            Self::BackdropDense => 0.97,
            Self::MediaEdge => 0.12,
            Self::FocusRing => 0.5,
            Self::SwatchEdge => 0.22,
            Self::TerminalDim => 0.6,
            Self::MentionGlyph => 0.85,
            Self::ChartRule => 0.3,
            Self::ChartSelection => 0.08,
            Self::AnnotationRule => 0.7,
            Self::EventRule => 0.55,
            Self::AnnotationBand => 0.06,
            Self::AreaStacked => 0.55,
            Self::Area => 0.22,
            Self::ReferenceLine => 0.5,
            Self::AreaQuiet => 0.14,
            Self::Overlap => 0.5,
            Self::BarSolo => 0.85,
            Self::BoxFill => 0.25,
            Self::Outlier => 0.8,
            Self::LegendHidden => 0.35,
            Self::LabelBacking => 0.7,
            Self::Dimmed => 0.45,
            Self::TreemapLeaf => 0.5,
            Self::TreemapLeafHover => 0.72,
            Self::TreemapGroup => 0.08,
            Self::TreemapEdge => 0.35,
            Self::TreemapHeader => 0.22,
            Self::TreemapFocusLeaf => 0.1,
            Self::TreemapFocusLeafHover => 0.144,
            Self::TreemapFocusGroup => 0.016,
            Self::TreemapFocusEdge => 0.07,
            Self::TreemapFocusHeader => 0.044,
            Self::TreemapFocusSwatch => 0.2,
            Self::NodeDimmed => 0.3,
            Self::Flow => 0.32,
            Self::FlowLit => 0.6,
            Self::FlowDimmed => 0.08,
            Self::LabelDimmed => 0.4,
            Self::Span => 0.55,
            Self::SpanHover => 0.75,
            Self::BadgeActive => 0.9,
            Self::BadgeTint => 0.18,
            Self::AnnotationEdge => 0.3,
            Self::NoteHover => 0.08,
            Self::CalloutTint => 0.07,
        }
    }
}

impl Oklch {
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
    #[must_use]
    pub const fn at(self, purpose: Opacity) -> Self {
        self.with_alpha(self.a * purpose.alpha())
    }
    #[must_use]
    pub const fn in_hue_of(self, family: Self) -> Self {
        Self {
            c: family.c,
            h: family.h,
            paint: None,
            ..self
        }
    }
    #[must_use]
    pub fn fade(self, amount: f32) -> Self {
        self.with_alpha(self.a * amount.clamp(0.0, 1.0))
    }
    #[must_use]
    pub fn animate_to(self, to: Self, progress: f32) -> Self {
        let t = progress.clamp(0.0, 1.0);
        if t <= 0.0 {
            return self;
        }
        if t >= 1.0 {
            return to;
        }
        let from = if self.c < 1e-4 { to.h } else { self.h };
        let target = if to.c < 1e-4 { from } else { to.h };
        let delta = (target - from + 540.0) % 360.0 - 180.0;
        Self::new(
            (to.l - self.l).mul_add(t, self.l),
            (to.c - self.c).mul_add(t, self.c),
            (from + delta * t).rem_euclid(360.0),
        )
        .with_alpha((to.a - self.a).mul_add(t, self.a))
    }
    #[must_use]
    pub const fn alpha(self) -> f32 {
        self.a
    }
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
    #[must_use]
    pub fn to_reference_paint(self) -> aqua::GpuiHsla {
        let hsla = self.to_palette_paint();
        aqua::GpuiHsla {
            h: hsla.hue.into_positive_degrees() / 360.0,
            s: hsla.saturation,
            l: hsla.lightness,
            a: hsla.alpha,
        }
    }

    #[must_use]
    pub fn to_reference_text_color(self) -> aqua::GpuiTextColor {
        let paint = self.to_palette_paint();
        aqua::GpuiTextColor {
            hue_degrees: paint.hue.into_raw_degrees(),
            saturation: paint.saturation,
            lightness: paint.lightness,
            alpha: paint.alpha,
        }
    }

    #[must_use]
    #[expect(
        clippy::float_cmp,
        reason = "cache validity requires the exact source components"
    )]
    pub fn to_palette_paint(self) -> palette::Hsla {
        use palette::IntoColor;
        match self.paint {
            Some((source, paint)) if source == [self.l, self.c, self.h, self.a] => paint,
            _ => {
                let color = self.to_srgb();
                palette::Srgba::new(color.r, color.g, color.b, color.a).into_color()
            }
        }
    }

    #[must_use]
    pub fn to_paint(self) -> aqua_core::Oklch {
        aqua_core::Oklch::new(self.l, self.c, self.h).with_alpha(self.a)
    }

    fn resolved(mut self) -> Self {
        self.paint = Some(([self.l, self.c, self.h, self.a], self.to_palette_paint()));
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThemeMode {
    Light,
    #[default]
    Dark,
}

impl ThemeMode {
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }
    #[must_use]
    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale([Oklch; 12]);

impl Scale {
    #[must_use]
    pub const fn step(&self, n: usize) -> Oklch {
        assert!(n >= 1 && n <= 12, "scale steps run 1..=12");
        self.0[n - 1]
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusScale {
    pub bg: Oklch,
    pub border: Oklch,
    pub solid: Oklch,
    pub text: Oklch,
    pub contrast: Oklch,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BannerColors {
    pub bg: Oklch,
    pub border: Oklch,
    pub text: Oklch,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxColors {
    pub comment: Oklch,
    pub string: Oklch,
    pub number: Oklch,
    pub keyword: Oklch,
    pub r#type: Oklch,
    pub function: Oklch,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnsiColors(pub [Oklch; 16]);
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTokens {
    pub mode: ThemeMode,
    pub gray: Scale,
    pub accent: Scale,
    pub accent_contrast: Oklch,
    pub red: StatusScale,
    pub amber: StatusScale,
    pub green: StatusScale,
    pub link: Oklch,
    pub black: Oklch,
    pub white: Oklch,
    pub scrim: Oklch,
    pub shadow: Oklch,
    pub palette: [Oklch; 8],
    pub claude: Oklch,
    pub ansi: AnsiColors,
    pub frame: Oklch,
    pub background: Oklch,
    pub surface: Oklch,
    pub raised: Oklch,
    pub hover: Oklch,
    pub selected: Oklch,
    pub pressed: Oklch,
    pub line: Oklch,
    pub line_strong: Oklch,
    pub text: Oklch,
    pub text_muted: Oklch,
    pub text_muted_glass: Oklch,
    pub text_faint: Oklch,
    pub ring: Oklch,
    pub text_selection: Oklch,
    pub primary: Oklch,
    pub primary_hover: Oklch,
    pub primary_text: Oklch,
    pub action_top: Oklch,
    pub action_bottom: Oklch,
    pub action_hover_top: Oklch,
    pub action_hover_bottom: Oklch,
    pub action_text: Oklch,
    pub destructive: Oklch,
    pub destructive_hover: Oklch,
    pub destructive_text: Oklch,
    pub destructive_contrast: Oklch,
    pub banner_info: BannerColors,
    pub banner_warning: BannerColors,
    pub banner_success: BannerColors,
    pub banner_error: BannerColors,
    pub favourite: Oklch,
    pub unread: Oklch,
    pub needs_answer: Oklch,
    pub question_tint: Oklch,
    pub thread_title_active: Oklch,
    pub mention_skill: Oklch,
    pub mention_connector: Oklch,
    pub mention_command: Oklch,
    pub syntax: SyntaxColors,
}
mod polar {
    type Lch = (f32, f32, f32);

    pub const WHITE: Lch = (1.0, 0.0, 0.0);
    pub const BLACK: Lch = (0.0, 0.0, 0.0);
    pub const GRAY_50: Lch = (0.985, 0.002, 247.839);
    pub const GRAY_100: Lch = (0.967, 0.003, 264.542);
    pub const GRAY_200: Lch = (0.928, 0.006, 264.531);
    pub const GRAY_300: Lch = (0.872, 0.01, 258.338);
    pub const GRAY_400: Lch = (0.707, 0.022, 261.325);
    pub const GRAY_500: Lch = (0.551, 0.027, 264.364);
    pub const GRAY_600: Lch = (0.446, 0.03, 256.802);
    pub const GRAY_900: Lch = (0.21, 0.034, 264.665);
    pub const POLAR_50: Lch = (0.8845, 0.0, 0.0);
    pub const POLAR_100: Lch = (0.8373, 0.0, 0.0);
    pub const POLAR_300: Lch = (0.7090, 0.0, 0.0);
    pub const POLAR_400: Lch = (0.6154, 0.0, 0.0);
    pub const POLAR_500: Lch = (0.5634, 0.0, 0.0);
    pub const POLAR_600: Lch = (0.2801, 0.0, 0.0);
    pub const POLAR_700: Lch = (0.2484, 0.0, 0.0);
    pub const POLAR_800: Lch = (0.2100, 0.0, 0.0);
    pub const POLAR_900: Lch = (0.1757, 0.0, 0.0);
    pub const POLAR_950: Lch = (0.1394, 0.0, 0.0);
    pub const GLASS_INK_DIM: Lch = (0.82, 0.012, 80.0);
    pub const EMERALD_50: Lch = (0.979, 0.021, 166.113);
    pub const EMERALD_300: Lch = (0.845, 0.143, 164.978);
    pub const EMERALD_400: Lch = (0.765, 0.177, 163.223);
    pub const EMERALD_500: Lch = (0.696, 0.17, 162.48);
    pub const EMERALD_600: Lch = (0.596, 0.145, 163.225);
    pub const EMERALD_700: Lch = (0.508, 0.118, 165.612);
    pub const EMERALD_950: Lch = (0.262, 0.051, 172.552);
    pub const RED_50: Lch = (0.971, 0.013, 17.38);
    pub const RED_300: Lch = (0.808, 0.114, 19.571);
    pub const RED_400: Lch = (0.704, 0.191, 22.216);
    pub const RED_500: Lch = (0.637, 0.237, 25.331);
    pub const RED_600: Lch = (0.577, 0.245, 27.325);
    pub const RED_700: Lch = (0.505, 0.213, 27.518);
    pub const RED_950: Lch = (0.258, 0.092, 26.042);
    pub const PINK_400: Lch = (0.718, 0.202, 349.761);
    pub const PINK_600: Lch = (0.592, 0.249, 0.584);
    pub const AMBER_50: Lch = (0.987, 0.022, 95.277);
    pub const AMBER_300: Lch = (0.879, 0.169, 91.605);
    pub const AMBER_400: Lch = (0.828, 0.189, 84.429);
    pub const AMBER_500: Lch = (0.769, 0.188, 70.08);
    pub const AMBER_600: Lch = (0.666, 0.179, 58.318);
    pub const AMBER_700: Lch = (0.555, 0.163, 48.998);
    pub const AMBER_950: Lch = (0.279, 0.077, 45.635);
    pub const INDIGO_50: Lch = (0.962, 0.018, 272.314);
    pub const INDIGO_100: Lch = (0.93, 0.034, 272.788);
    pub const INDIGO_200: Lch = (0.87, 0.065, 274.039);
    pub const INDIGO_300: Lch = (0.785, 0.115, 274.713);
    pub const INDIGO_400: Lch = (0.673, 0.182, 276.935);
    pub const INDIGO_500: Lch = (0.585, 0.233, 277.117);
    pub const INDIGO_600: Lch = (0.511, 0.262, 276.966);
    pub const INDIGO_700: Lch = (0.457, 0.24, 277.023);
    pub const INDIGO_800: Lch = (0.398, 0.195, 277.366);
    pub const INDIGO_900: Lch = (0.359, 0.144, 278.697);
    pub const INDIGO_950: Lch = (0.257, 0.09, 281.288);
    pub const BLUE_100: Lch = (0.932, 0.032, 255.585);
    pub const BLUE_200: Lch = (0.882, 0.059, 254.128);
    pub const BLUE_300: Lch = (0.809, 0.105, 251.813);
    pub const BLUE_400: Lch = (0.707, 0.165, 254.624);
    pub const SKY_400: Lch = (0.676, 0.075, 250.22);
    pub const SKY_700: Lch = (0.497, 0.106, 256.73);
    pub const BLUE_500: Lch = (0.623, 0.214, 259.815);
    pub const BLUE_600: Lch = (0.546, 0.245, 262.881);
    pub const BLUE_700: Lch = (0.488, 0.243, 264.376);
    pub const BLUE_900: Lch = (0.379, 0.146, 265.522);
    pub const PURPLE_300: Lch = (0.827, 0.119, 306.383);
    pub const PURPLE_400: Lch = (0.714, 0.203, 305.504);
    pub const PURPLE_600: Lch = (0.558, 0.288, 302.321);
    pub const PURPLE_700: Lch = (0.496, 0.265, 301.924);
    pub const CYAN_300: Lch = (0.865, 0.127, 207.078);
    pub const CYAN_400: Lch = (0.789, 0.154, 211.53);
    pub const CYAN_600: Lch = (0.609, 0.126, 221.723);
    pub const CYAN_700: Lch = (0.52, 0.105, 223.128);
    pub const CLAUDE_CLAY: Lch = (0.6724, 0.1308, 38.76);
    pub const CHART: [Lch; 8] = [
        (0.7019, 0.1540, 32.23),
        (0.7979, 0.1127, 59.06),
        (0.8418, 0.0902, 72.08),
        (0.8204, 0.0834, 167.10),
        (0.6482, 0.1443, 264.09),
        (0.4236, 0.1469, 266.98),
        (0.7578, 0.1234, 357.99),
        (0.8353, 0.0078, 17.31),
    ];
}
fn in_gamut((l, c, h): (f32, f32, f32)) -> (f32, f32, f32) {
    let fits = |c: f32| {
        color::AlphaColor::<color::Oklch>::new([l, c, h, 1.0])
            .convert::<color::Srgb>()
            .components[..3]
            .iter()
            .all(|channel| (-1e-4..=1.0 + 1e-4).contains(channel))
    };
    if fits(c) {
        return (l, c, h);
    }
    let (mut low, mut high) = (0.0_f32, c);
    for _ in 0..24 {
        let mid = f32::midpoint(low, high);
        if fits(mid) {
            low = mid;
        } else {
            high = mid;
        }
    }
    (l, low, h)
}

fn token(lch: (f32, f32, f32)) -> Oklch {
    let (l, c, h) = in_gamut(lch);
    Oklch::new(l, c, h).resolved()
}

impl ColorTokens {
    fn build(mode: ThemeMode) -> Self {
        use polar::{
            AMBER_50, AMBER_300, AMBER_400, AMBER_500, AMBER_600, AMBER_700, AMBER_950, BLACK,
            BLUE_100, BLUE_200, BLUE_300, BLUE_400, BLUE_500, BLUE_600, BLUE_700, BLUE_900, CHART,
            CLAUDE_CLAY, CYAN_300, CYAN_400, CYAN_600, CYAN_700, EMERALD_50, EMERALD_300,
            EMERALD_400, EMERALD_500, EMERALD_600, EMERALD_700, EMERALD_950, GLASS_INK_DIM, GRAY_50, GRAY_100,
            GRAY_200, GRAY_300, GRAY_400, GRAY_500, GRAY_600, GRAY_900, INDIGO_50, INDIGO_100,
            INDIGO_200, INDIGO_300, INDIGO_400, INDIGO_500, INDIGO_600, INDIGO_700, INDIGO_800,
            INDIGO_900, INDIGO_950, PINK_400, PINK_600, POLAR_50, POLAR_100, POLAR_300, POLAR_400,
            POLAR_500, POLAR_600, POLAR_700, POLAR_800, POLAR_900, POLAR_950, PURPLE_300,
            PURPLE_400, PURPLE_600, PURPLE_700, RED_50, RED_300, RED_400, RED_500, RED_600,
            RED_700, RED_950, SKY_400, SKY_700, WHITE,
        };
        let dark = mode.is_dark();
        let pick = |light: (f32, f32, f32), dark_value: (f32, f32, f32)| {
            token(if dark { dark_value } else { light })
        };
        let ramp = |light: [(f32, f32, f32); 12], dark_value: [(f32, f32, f32); 12]| {
            Scale(if dark { dark_value } else { light }.map(token))
        };
        let gray = ramp(
            [
                WHITE, GRAY_50, GRAY_100, GRAY_200, GRAY_200, GRAY_200, GRAY_300, GRAY_400,
                GRAY_500, GRAY_600, GRAY_500, GRAY_900,
            ],
            [
                POLAR_950, POLAR_900, POLAR_800, POLAR_700, POLAR_600, POLAR_700, POLAR_600,
                POLAR_500, POLAR_500, POLAR_400, POLAR_400, POLAR_50,
            ],
        );
        let accent = ramp(
            [
                INDIGO_50, INDIGO_50, INDIGO_50, INDIGO_100, INDIGO_100, INDIGO_200, INDIGO_300,
                INDIGO_300, INDIGO_500, INDIGO_600, INDIGO_500, INDIGO_900,
            ],
            [
                INDIGO_950, INDIGO_950, INDIGO_950, INDIGO_900, INDIGO_900, INDIGO_800, INDIGO_700,
                INDIGO_700, INDIGO_500, INDIGO_400, INDIGO_500, INDIGO_100,
            ],
        );
        let accent_contrast = token(WHITE);
        let red = StatusScale {
            bg: pick(RED_50, RED_950),
            border: pick(RED_300, RED_700),
            solid: pick(RED_500, RED_600),
            text: token(RED_500),
            contrast: token(WHITE),
        };
        let amber = StatusScale {
            bg: pick(AMBER_50, AMBER_950),
            border: pick(AMBER_300, AMBER_700),
            solid: token(AMBER_500),
            text: token(AMBER_500),
            contrast: token(AMBER_950),
        };
        let green = StatusScale {
            bg: pick(EMERALD_50, EMERALD_950),
            border: pick(EMERALD_300, EMERALD_700),
            solid: pick(EMERALD_500, EMERALD_600),
            text: token(EMERALD_500),
            contrast: token(EMERALD_950),
        };
        let link = pick(SKY_700, SKY_400);
        let black = token(BLACK);
        let white = token(WHITE);
        let banner = |scale: StatusScale| BannerColors {
            bg: scale.bg,
            border: scale.border,
            text: scale.text,
        };
        let text_faint = pick(GRAY_400, POLAR_500);
        let ansi = if dark {
            [
                POLAR_700,
                RED_400,
                EMERALD_400,
                AMBER_400,
                BLUE_400,
                PURPLE_400,
                CYAN_400,
                POLAR_100,
                POLAR_500,
                RED_300,
                EMERALD_300,
                AMBER_300,
                BLUE_300,
                PURPLE_300,
                CYAN_300,
                WHITE,
            ]
        } else {
            [
                GRAY_900,
                RED_600,
                EMERALD_600,
                AMBER_600,
                BLUE_600,
                PURPLE_600,
                CYAN_600,
                GRAY_200,
                GRAY_500,
                RED_700,
                EMERALD_700,
                AMBER_700,
                BLUE_700,
                PURPLE_700,
                CYAN_700,
                GRAY_50,
            ]
        };
        Self {
            mode,
            gray,
            accent,
            accent_contrast,
            red,
            amber,
            green,
            link,
            black,
            white,
            scrim: black.with_alpha(0.7).resolved(),
            shadow: black.with_alpha(0.06).resolved(),
            palette: CHART.map(token),
            claude: token(CLAUDE_CLAY),
            ansi: AnsiColors(ansi.map(token)),
            frame: pick(GRAY_100, POLAR_950),
            background: pick(WHITE, POLAR_900),
            surface: pick(GRAY_50, POLAR_800),
            raised: pick(GRAY_100, POLAR_800),
            hover: pick(GRAY_200, POLAR_700),
            selected: pick(GRAY_100, POLAR_700),
            pressed: pick(GRAY_200, POLAR_600),
            line: pick(GRAY_200, POLAR_700),
            line_strong: pick(GRAY_300, POLAR_600),
            text: pick(BLACK, WHITE),
            text_muted: pick(GRAY_500, POLAR_300),
            text_muted_glass: pick(GRAY_500, GLASS_INK_DIM),
            text_faint,
            ring: pick(BLUE_300, BLUE_700),
            text_selection: pick(BLUE_100, BLUE_900),
            primary: pick(BLACK, WHITE),
            primary_hover: pick(POLAR_600, POLAR_50),
            primary_text: pick(WHITE, BLACK),
            action_top: token(BLUE_500),
            action_bottom: token(BLUE_700),
            action_hover_top: token(BLUE_400),
            action_hover_bottom: token(BLUE_600),
            action_text: token(WHITE),
            destructive: red.solid,
            destructive_hover: pick(RED_600, RED_500),
            destructive_text: red.text,
            destructive_contrast: red.contrast,
            banner_info: BannerColors {
                bg: pick(INDIGO_50, INDIGO_950),
                border: pick(INDIGO_300, INDIGO_700),
                text: token(INDIGO_500),
            },
            banner_warning: banner(amber),
            banner_success: banner(green),
            banner_error: banner(red),
            favourite: amber.text,
            unread: link,
            needs_answer: pick(PINK_600, PINK_400),
            question_tint: pick(INDIGO_50, INDIGO_950),
            thread_title_active: link,
            mention_skill: link,
            mention_connector: pick(GRAY_500, POLAR_300),
            mention_command: amber.text,
            syntax: SyntaxColors {
                comment: text_faint,
                string: pick(EMERALD_600, EMERALD_300),
                number: pick(AMBER_600, AMBER_300),
                keyword: pick(PURPLE_600, PURPLE_300),
                r#type: pick(GRAY_900, POLAR_50),
                function: pick(BLUE_600, BLUE_200),
            },
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypographyTokens {
    pub fonts: FontSet,
    pub sans: FontRole,
    pub mono: FontRole,
    pub heading: FontRole,
    pub tracking_em: f32,
    pub control_text: Pixels,
    pub editor_text_desktop: Pixels,
    pub label_text: Pixels,
}
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TextStep {
    X3s,
    X2s,
    Xs,
    Terminal,
    XsMid,
    Sm,
    SmMid,
    Base,
    Lg,
    Xl,
    X2l,
}

impl TypographyTokens {
    #[must_use]
    pub const fn size(step: TextStep) -> Pixels {
        match step {
            TextStep::X3s => px(10.0),
            TextStep::X2s => px(11.0),
            TextStep::Xs => px(12.0),
            TextStep::Terminal => px(12.5),
            TextStep::XsMid => px(13.0),
            TextStep::Sm => px(14.0),
            TextStep::SmMid => px(15.0),
            TextStep::Base => px(16.0),
            TextStep::Lg => px(18.0),
            TextStep::Xl => px(20.0),
            TextStep::X2l => px(22.0),
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FontSet {
    #[default]
    Spline,
    System,
}

impl FontSet {
    pub const ALL: [Self; 2] = [Self::System, Self::Spline];
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Spline => "spline",
            Self::System => "system",
        }
    }
    #[must_use]
    pub fn from_key(key: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|fonts| Some(fonts.key()) == key)
            .unwrap_or_default()
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Spline => "Spline",
            Self::System => "System",
        }
    }

    const fn sans(self) -> &'static str {
        match self {
            Self::Spline => "Spline Sans",
            Self::System if cfg!(target_os = "windows") => "Segoe UI",
            Self::System => ".SystemUIFont",
        }
    }

    const fn mono(self) -> &'static str {
        match self {
            Self::Spline => "Spline Sans Mono",
            Self::System if cfg!(target_os = "windows") => "Cascadia Mono",
            Self::System if cfg!(target_os = "macos") => "Menlo",
            Self::System => "DejaVu Sans Mono",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontRole {
    pub family: &'static str,
}
pub struct ProseTypography;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProseHeading {
    pub size_px: f32,
    pub line_px: f32,
    pub tracking_px: f32,
    pub margin_top_px: f32,
    pub margin_bottom_px: f32,
}

impl ProseTypography {
    pub const BODY_SIZE_PX: f32 = 16.0;
    pub const BODY_LINE_PX: f32 = 28.0;
    pub const BODY_WEIGHT: FontWeight = FontWeight(410);
    pub const BODY_TRACKING_PX: f32 = -0.64;
    pub const TRACKING_EM: f32 = -0.04;
    #[must_use]
    pub const fn body_tracking_px(size_px: f32) -> f32 {
        size_px * Self::TRACKING_EM
    }
    pub const HEADING_WEIGHT: FontWeight = FontWeight(630);
    pub const STRONG_WEIGHT: FontWeight = FontWeight(600);
    pub const LINK_WEIGHT: FontWeight = FontWeight::MEDIUM;
    pub const CODE_SIZE_PX: f32 = 14.0;
    pub const CODE_LINE_PX: f32 = 24.0;
    pub const CODE_PAD_PX: f32 = 16.0;
    pub const PARAGRAPH_MARGIN_PX: f32 = 20.0;
    pub const FENCE_MARGIN_PX: f32 = 24.0;
    pub const LIST_MARGIN_PX: f32 = 20.0;
    pub const LIST_INDENT_PX: f32 = 26.0;
    pub const ITEM_GAP_PX: f32 = 8.0;
    pub const ITEM_PARAGRAPH_MARGIN_PX: f32 = 12.0;
    pub const NESTED_LIST_MARGIN_PX: f32 = 12.0;
    pub const TABLE_SIZE_PX: f32 = 14.0;
    pub const TABLE_LINE_PX: f32 = 24.0;
    pub const TABLE_MARGIN_PX: f32 = 28.0;
    pub const TABLE_CELL_PAD_PX: f32 = 8.0;
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpacingTokens;

impl SpacingTokens {
    pub const BASE_PX: f32 = 4.0;
    #[must_use]
    pub fn steps(self, count: f32) -> Pixels {
        px(Self::BASE_PX * count)
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RadiusStep {
    X3s,
    X2s,
    X2sMid,
    Xs,
    Sm,
    Md,
    Lg,
    LgMid,
    Xl,
    XlMid,
    X2l,
    X2lMid,
    X3l,
    X4l,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RadiusTokens;

impl RadiusTokens {
    pub const BASE_PX: f32 = 10.0;
    #[must_use]
    pub const fn value(step: RadiusStep) -> Pixels {
        match step {
            RadiusStep::X3s => px(1.0),
            RadiusStep::X2s => px(2.0),
            RadiusStep::X2sMid => px(3.0),
            RadiusStep::Xs => px(4.0),
            RadiusStep::Sm => px(6.0),
            RadiusStep::Md => px(8.0),
            RadiusStep::Lg => px(10.0),
            RadiusStep::LgMid => px(12.0),
            RadiusStep::Xl => px(14.0),
            RadiusStep::XlMid => px(16.0),
            RadiusStep::X2l => px(18.0),
            RadiusStep::X2lMid => px(20.0),
            RadiusStep::X3l => px(22.0),
            RadiusStep::X4l => px(26.0),
        }
    }
    #[must_use]
    pub fn nested(surface: Pixels, gap: Pixels) -> Pixels {
        let inner = f32::from(surface) - f32::from(gap);
        px(inner.max(0.0))
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowLayer {
    pub color: Oklch,
    pub offset_x: Pixels,
    pub offset_y: Pixels,
    pub blur_radius: Pixels,
    pub spread_radius: Pixels,
}

impl From<ShadowLayer> for aqua::ShadowLayer {
    fn from(layer: ShadowLayer) -> Self {
        Self {
            dx: layer.offset_x,
            dy: layer.offset_y,
            blur: layer.blur_radius,
            spread: layer.spread_radius,
            inset: false,
            color: layer.color.to_paint(),
        }
    }
}

impl From<ShadowLayer> for aqua_ui::style::GpuiShadowLayer {
    fn from(layer: ShadowLayer) -> Self {
        Self {
            dx: layer.offset_x,
            dy: layer.offset_y,
            blur: layer.blur_radius,
            spread: layer.spread_radius,
            inset: false,
            color: layer.color.to_reference_paint(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevationTokens {
    pub hairline: ShadowLayer,
    pub raised: [ShadowLayer; 3],
    pub floating: [ShadowLayer; 3],
    pub overlay: [ShadowLayer; 4],
}

impl ElevationTokens {
    #[must_use]
    pub const fn drops(&self, elevation: Elevation) -> &[ShadowLayer] {
        match elevation {
            Elevation::Raised => &self.raised,
            Elevation::Floating => &self.floating,
            Elevation::Overlay => &self.overlay,
        }
    }
    fn build(mode: ThemeMode, colors: &ColorTokens) -> Self {
        let shadow = |color: Oklch, strength: f32, oy: f32, blur: f32, spread: f32| ShadowLayer {
            color: color.with_alpha(strength).resolved(),
            offset_x: px(0.0),
            offset_y: px(oy),
            blur_radius: px(blur),
            spread_radius: px(spread),
        };
        let (ink, edge, drop) = match mode {
            ThemeMode::Light => (colors.gray.step(12), 0.07, 1.0),
            ThemeMode::Dark => (colors.black, 0.10, 5.0),
        };
        let d = |strength: f32, oy: f32, blur: f32, spread: f32| {
            shadow(ink, (strength * drop).min(0.6), oy, blur, spread)
        };
        Self {
            hairline: shadow(colors.gray.step(12), edge, 0.0, 0.0, 0.5),
            raised: [
                shadow(colors.white, 0.08, -0.5, 0.0, 0.0),
                d(0.05, 1.0, 2.0, 0.0),
                d(0.04, 2.0, 4.0, -1.0),
            ],
            floating: [
                d(0.04, 1.0, 2.0, 0.0),
                d(0.05, 4.0, 8.0, -2.0),
                d(0.06, 12.0, 24.0, -8.0),
            ],
            overlay: [
                d(0.05, 1.0, 2.0, 0.0),
                d(0.06, 4.0, 8.0, -2.0),
                d(0.07, 12.0, 24.0, -6.0),
                d(0.07, 24.0, 48.0, -12.0),
            ],
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Elevation {
    Raised,
    Floating,
    Overlay,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DensityTokens {
    pub control_default: Pixels,
    pub command_list_max_height: Pixels,
    pub control_sm: Pixels,
    pub switch_default: (Pixels, Pixels),
    pub switch_sm: (Pixels, Pixels),
    pub badge_height: Pixels,
    pub card_padding_compact: Pixels,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteractionTokens {
    pub focus_ring_width: Pixels,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtisanTheme {
    pub colors: ColorTokens,
    pub typography: TypographyTokens,
    pub spacing: SpacingTokens,
    pub radius: RadiusTokens,
    pub elevation: ElevationTokens,
    pub density: DensityTokens,
    pub interaction: InteractionTokens,
}

impl TypographyTokens {
    #[must_use]
    pub const fn new(fonts: FontSet) -> Self {
        Self {
            fonts,
            sans: FontRole {
                family: fonts.sans(),
            },
            mono: FontRole {
                family: fonts.mono(),
            },
            heading: FontRole {
                family: fonts.sans(),
            },
            tracking_em: match fonts {
                FontSet::Spline => ProseTypography::TRACKING_EM,
                FontSet::System => 0.0,
            },
            control_text: px(14.0),
            editor_text_desktop: px(14.0),
            label_text: px(12.0),
        }
    }
    #[must_use]
    pub const fn heading_tracking_px(&self, heading: &ProseHeading) -> f32 {
        match self.fonts {
            FontSet::Spline => heading.tracking_px,
            FontSet::System => 0.0,
        }
    }
}
impl ArtisanTheme {
    #[must_use]
    pub fn resolved() -> Self {
        *Self::for_mode(ThemeMode::Dark)
    }
    #[must_use]
    pub fn for_mode(mode: ThemeMode) -> &'static Self {
        static LIGHT: OnceLock<ArtisanTheme> = OnceLock::new();
        static DARK: OnceLock<ArtisanTheme> = OnceLock::new();
        match mode {
            ThemeMode::Light => LIGHT.get_or_init(|| Self::build(mode)),
            ThemeMode::Dark => DARK.get_or_init(|| Self::build(mode)),
        }
    }

    fn build(mode: ThemeMode) -> Self {
        let colors = ColorTokens::build(mode);
        Self {
            colors,
            typography: TypographyTokens::new(FontSet::Spline),
            spacing: SpacingTokens,
            radius: RadiusTokens,
            elevation: ElevationTokens::build(mode, &colors),
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
            },
        }
    }
}
