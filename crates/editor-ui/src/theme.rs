//! Typed application theme: fixed colour tokens for light and dark, resolved
//! once per mode. OKLCH source colours retain cached GPUI paint values.
//!
//! Every colour the UI paints is a token from this module. Opacity never makes
//! a new shade of a colour; it only serves the purposes named by [`Opacity`],
//! plus animation fades ([`Oklch::fade`]).

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

/// An OKLCH token colour with a cached GPUI paint conversion. Only this
/// module makes new colours; the rest of the app reads them from the theme.
#[derive(Clone, Copy, Debug)]
pub struct Oklch {
    pub(crate) l: f32,
    pub(crate) c: f32,
    pub(crate) h: f32,
    pub(crate) a: f32,
    paint: Option<([f32; 4], Hsla)>,
}

impl PartialEq for Oklch {
    fn eq(&self, other: &Self) -> bool {
        self.l == other.l && self.c == other.c && self.h == other.h && self.a == other.a
    }
}

/// What opacity is for. Opacity never makes a lighter or darker shade of a
/// colour; every shade is its own token.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Opacity {
    /// A control, row or item that can't be used right now.
    Disabled,
    /// A material laid over content the app doesn't control (the composer).
    Glass,
    /// The stronger material of floating pickers, menus and tooltips.
    GlassStrong,
    /// Nothing: the far end of a fade.
    Clear,
    /// The near corner of the quiet glass face (composer, badges).
    GlassFaceQuiet,
    /// The far corner of the quiet glass face.
    GlassFaceQuietFar,
    /// The near corner of the strong glass face (pickers, tooltips).
    GlassFaceStrong,
    /// The far corner of the strong glass face.
    GlassFaceStrongFar,
    /// The faint text-coloured base laid under a glass face.
    GlassBase,
    /// The tabbed card's shell and strip panes over the transcript.
    GlassPane,
    /// The lit corner of the tabbed card's strip: a step-8 fill thinned to
    /// the old S700 pane, so the strip stands a step above the shell.
    GlassPaneLit,
    /// The faintest top sheen on a glass pane (tab card faces).
    SheenFaint,
    /// A soft top sheen (tab card edge light).
    SheenSoft,
    /// The top sheen of quiet glass.
    Sheen,
    /// The top sheen of strong glass.
    SheenStrong,
    /// The bright outer top rim of a glass card.
    RimLight,
    /// The inner top rim light of a glass card.
    RimInner,
    /// The inner bottom rim shade of a glass card.
    RimShade,
    /// A hairline rule or ring drawn over glass or varying backdrops.
    Hairline,
    /// The faintest hairline ring (tab card edge).
    HairlineFaint,
    /// The top of the hover pill, laid over glass and varying surfaces.
    HoverTop,
    /// The bottom of the hover pill.
    HoverBottom,
    /// The hairline ring around the hover pill and chart hover bands.
    HoverRing,
    /// The border of a glass surface: a text-coloured edge over any backdrop.
    GlassBorder,
    /// The tinted fill behind a status chip or toned badge, in the tone's colour.
    ToneTint,
    /// An icon inside a coloured element: the element's text colour, slightly quieter.
    ToneIcon,
    /// Hairline rules, axes and outlines of rich content drawn over cards and data.
    Rule,
    /// A harness's colour laid over a message bubble its agent sent, leaning the fill toward it.
    EngineLean,
    /// The hairline edge in a harness's colour around a message its agent sent.
    EngineEdge,
    /// The unlit segments of a usage meter, drawn over a glass menu.
    MeterTrack,
    /// The unfilled track of a progress bar on a glass card.
    ProgressTrack,
    /// A picker tab whose engine needs installing or signing in first.
    Unavailable,
    /// The page-coloured backdrop a dialog lays over the app behind it.
    Backdrop,
    /// The black shade behind a bare dialog (image viewer), over any content.
    Shade,
    /// The near-opaque page backdrop behind the full-window image preview.
    BackdropDense,
    /// The hairline edge of a media frame, drawn against image content.
    MediaEdge,
    /// The focus halo around a focused control, over whatever surface holds it.
    FocusRing,
    /// The text-coloured edge around an inline colour swatch, against the swatch's own colour.
    SwatchEdge,
    /// Dim (SGR 2) terminal text in a colour the program chose: that colour, dimmed.
    TerminalDim,
    /// The icon glyph inside a mention pill, a touch quieter than the mention's text.
    MentionGlyph,
    /// The hover rule drawn across a chart's data.
    ChartRule,
    /// The drag-to-zoom selection band laid over a chart's data.
    ChartSelection,
    /// An annotation's rule over chart data, in the muted text colour.
    AnnotationRule,
    /// An event annotation's solid rule (and chart markers) over chart data.
    EventRule,
    /// An annotated range's band laid over chart data.
    AnnotationBand,
    /// A stacked area's fill, so the series beneath still reads.
    AreaStacked,
    /// An overlapping area's fill, so every series' area shows through the others.
    Area,
    /// A dashed reference line (equal share) drawn over a chart's fill, in the muted text colour.
    ReferenceLine,
    /// The quiet fill under a single curve, over the chart grid.
    AreaQuiet,
    /// Bars of several series overlaid in one bin, so each shows through the others.
    Overlap,
    /// A lone series' bars, a touch translucent over the grid.
    BarSolo,
    /// The box of a box plot, over its whisker and the grid.
    BoxFill,
    /// An outlier dot in a box plot, over the lane.
    Outlier,
    /// A legend entry whose series is hidden.
    LegendHidden,
    /// The page-coloured backing behind a label placed over chart data.
    LabelBacking,
    /// Chart items stepped back while another is hovered.
    Dimmed,
    /// A hierarchy leaf's fill in its branch colour.
    TreemapLeaf,
    /// A hovered hierarchy leaf's fill.
    TreemapLeafHover,
    /// A hierarchy group's faint fill in its branch colour.
    TreemapGroup,
    /// A hierarchy group's edge in its branch colour.
    TreemapEdge,
    /// A hierarchy group's header strip in its branch colour.
    TreemapHeader,
    /// A leaf of the focused node itself, in the text colour.
    TreemapFocusLeaf,
    /// A hovered leaf of the focused node itself.
    TreemapFocusLeafHover,
    /// The focused node's own group fill, in the text colour.
    TreemapFocusGroup,
    /// The focused node's own group edge.
    TreemapFocusEdge,
    /// The focused node's own header strip.
    TreemapFocusHeader,
    /// The readout swatch for the focused hierarchy node itself, in the text colour.
    TreemapFocusSwatch,
    /// A sankey node unrelated to the hovered one, stepped back.
    NodeDimmed,
    /// A sankey flow at rest, translucent so crossing flows show through.
    Flow,
    /// A sankey flow lit by the hover.
    FlowLit,
    /// A sankey flow unrelated to the hover, stepped back.
    FlowDimmed,
    /// A chart label unrelated to the hovered item, stepped back.
    LabelDimmed,
    /// A timeline span bar in its series colour, over the lane grid.
    Span,
    /// A hovered timeline span bar.
    SpanHover,
    /// A filled number badge in a tone's colour (the active annotation).
    BadgeActive,
    /// A number badge's tinted fill in a tone's colour at rest.
    BadgeTint,
    /// The left edge of a code line that owns an annotation, in the accent tone.
    AnnotationEdge,
    /// The tinted fill under a hovered annotation note.
    NoteHover,
    /// The tinted fill of a callout in its tone's colour.
    CalloutTint,
}

impl Opacity {
    /// The alpha this purpose paints at.
    #[must_use]
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per purpose so each alpha can be tuned on its own"
    )]
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
    /// Constructs an opaque OKLCH color.
    #[must_use]
    pub(crate) const fn new(l: f32, c: f32, h: f32) -> Self {
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
    pub(crate) const fn with_alpha(mut self, a: f32) -> Self {
        self.a = a;
        if let Some((mut source, mut paint)) = self.paint {
            source[3] = a;
            paint.alpha = a;
            self.paint = Some((source, paint));
        }
        self
    }

    /// This token painted for one of the purposes opacity serves.
    #[must_use]
    pub const fn at(self, purpose: Opacity) -> Self {
        self.with_alpha(self.a * purpose.alpha())
    }

    /// This token's lightness and alpha in `family`'s hue. The family's
    /// chroma comes along: a neutral token has none to carry a hue.
    #[must_use]
    pub const fn in_hue_of(self, family: Self) -> Self {
        Self {
            c: family.c,
            h: family.h,
            paint: None,
            ..self
        }
    }

    /// This token part-way through a fade in or out: `amount` is the
    /// animation's current visibility, `0` gone and `1` fully shown. Only
    /// for values that change over time, never a fixed shade.
    #[must_use]
    pub fn fade(self, amount: f32) -> Self {
        self.with_alpha(self.a * amount.clamp(0.0, 1.0))
    }

    /// Part-way from this token to `to` while an animation moves between
    /// them; `progress` runs `0..=1`. Never a resting colour.
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

    /// The token's alpha.
    #[must_use]
    pub const fn alpha(self) -> f32 {
        self.a
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
    #[expect(
        clippy::float_cmp,
        reason = "the cached paint is valid only for the exact source it was built from"
    )]
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

/// Light or dark.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThemeMode {
    /// Dark text on light surfaces.
    Light,
    /// Light text on dark surfaces.
    #[default]
    Dark,
}

impl ThemeMode {
    /// The other mode.
    #[must_use]
    pub const fn toggled(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }

    /// Whether this is the dark mode.
    #[must_use]
    pub const fn is_dark(self) -> bool {
        matches!(self, Self::Dark)
    }
}

/// A 12-step scale. Steps 1–2 are backgrounds, 3–5 component fills (rest,
/// hover, pressed), 6–8 borders and focus rings, 9–10 solid fills and
/// 11–12 text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale([Oklch; 12]);

impl Scale {
    /// Step `n` of the scale, `1..=12`.
    ///
    /// # Panics
    ///
    /// `n` is outside `1..=12`.
    #[must_use]
    pub const fn step(&self, n: usize) -> Oklch {
        assert!(n >= 1 && n <= 12, "scale steps run 1..=12");
        self.0[n - 1]
    }
}

/// A status hue: steps 3, 6, 9 and 11 of its scale plus the text colour
/// that reads on its solid step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StatusScale {
    /// Step 3: a tinted background (banners, badges, diff lines).
    pub bg: Oklch,
    /// Step 6: the border around that background.
    pub border: Oklch,
    /// Step 9: a solid fill (buttons, dots, bars).
    pub solid: Oklch,
    /// Step 11: text and icons in this hue.
    pub text: Oklch,
    /// Text and icons on [`Self::solid`].
    pub contrast: Oklch,
}

/// A banner's three colours: background, border, and text and icons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BannerColors {
    /// `--banner-*-bg`.
    pub bg: Oklch,
    /// `--banner-*-border`.
    pub border: Oklch,
    /// `--banner-*-text`, also its icons.
    pub text: Oklch,
}

/// Code highlighting roles.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SyntaxColors {
    /// `--syntax-comment`.
    pub comment: Oklch,
    /// `--syntax-string`.
    pub string: Oklch,
    /// `--syntax-number`.
    pub number: Oklch,
    /// `--syntax-keyword`.
    pub keyword: Oklch,
    /// `--syntax-type`.
    pub r#type: Oklch,
    /// `--syntax-function`.
    pub function: Oklch,
}

/// The sixteen terminal colours, in ANSI order: black, red, green, yellow,
/// blue, magenta, cyan, white, then the bright eight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnsiColors(pub [Oklch; 16]);

/// Every colour token. The scales and fixed colours are the palette; the
/// fields below them are named roles, each one of those tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTokens {
    /// Which mode these tokens belong to.
    pub mode: ThemeMode,
    /// `--gray-1` … `--gray-12`.
    pub gray: Scale,
    /// `--accent-1` … `--accent-12`.
    pub accent: Scale,
    /// `--accent-contrast`: text and icons on accent steps 9–10.
    pub accent_contrast: Oklch,
    /// `--red-*`.
    pub red: StatusScale,
    /// `--amber-*`.
    pub amber: StatusScale,
    /// `--green-*`.
    pub green: StatusScale,
    /// `--link`: links, and anything that reads as "open" or "chosen" text.
    pub link: Oklch,
    /// `--black`.
    pub black: Oklch,
    /// White: the light that catches glass and card edges (always white,
    /// in both modes).
    pub white: Oklch,
    /// `--scrim`: the backdrop behind a modal dialog.
    pub scrim: Oklch,
    /// `--shadow-color`.
    pub shadow: Oklch,
    /// `--palette-1` … `--palette-8`: data colours (chart series, avatars),
    /// never UI.
    pub palette: [Oklch; 8],
    /// Claude's clay: a brand colour the theme never changes.
    pub claude: Oklch,
    /// `--ansi-*`: terminal output.
    pub ansi: AnsiColors,

    /// The window frame around the content card: the titlebar and the
    /// rail, darker than the card in both modes.
    pub frame: Oklch,
    /// The content card the screens draw on.
    pub background: Oklch,
    /// Cards, fields and the composer.
    pub surface: Oklch,
    /// Popovers, menus, and a component's resting fill.
    pub raised: Oklch,
    /// What the pointer is over.
    pub hover: Oklch,
    /// The chosen row of a list or menu.
    pub selected: Oklch,
    /// A fill while pressed.
    pub pressed: Oklch,
    /// Hairlines, dividers and field borders.
    pub line: Oklch,
    /// Borders that must hold on any surface.
    pub line_strong: Oklch,
    /// Primary text and icons.
    pub text: Oklch,
    /// Secondary text and icons.
    pub text_muted: Oklch,
    /// Placeholders, comments and other faint text.
    pub text_faint: Oklch,
    /// Accent 8: the focus ring.
    pub ring: Oklch,
    /// Accent 6: selected text.
    pub text_selection: Oklch,
    /// Accent 9: the primary button.
    pub primary: Oklch,
    /// Accent 10: the primary button under the pointer.
    pub primary_hover: Oklch,
    /// `--accent-contrast`: text on the primary button.
    pub primary_text: Oklch,
    /// The accent button's fill, lit from above: lighter blue at the top,
    /// darker at the foot.
    pub action_top: Oklch,
    pub action_bottom: Oklch,
    /// The accent button under the pointer, a step lighter.
    pub action_hover_top: Oklch,
    pub action_hover_bottom: Oklch,
    /// Text and icons on the accent button.
    pub action_text: Oklch,
    /// `--destructive`: red 9, solid fills.
    pub destructive: Oklch,
    /// `--destructive-text`: red 11, error text and icons.
    pub destructive_text: Oklch,
    /// `--destructive-contrast`: text on a destructive fill.
    pub destructive_contrast: Oklch,
    /// `--banner-info-*`.
    pub banner_info: BannerColors,
    /// `--banner-warning-*`.
    pub banner_warning: BannerColors,
    /// `--banner-success-*`.
    pub banner_success: BannerColors,
    /// `--banner-error-*`.
    pub banner_error: BannerColors,
    /// `--favourite`: a star.
    pub favourite: Oklch,
    /// `--unread`: a thread with news.
    pub unread: Oklch,
    /// `--question-tint`: a question waiting on the user.
    pub question_tint: Oklch,
    /// `--thread-title-active`: the open thread's title.
    pub thread_title_active: Oklch,
    /// `--mention-skill`.
    pub mention_skill: Oklch,
    /// `--mention-connector`.
    pub mention_connector: Oklch,
    /// `--mention-command`.
    pub mention_command: Oklch,
    /// `--syntax-*`.
    pub syntax: SyntaxColors,
}

/// Polar's colour primitives, copied from polarsource/polar: the Orbit design
/// system's value tier (`clients/packages/orbit/src/tokens/value.stylex.ts`),
/// the web app's `@theme` (`clients/apps/web/src/styles/globals.css`) and the
/// dashboard's chart colours. Tailwind values are its OKLCH source; the
/// `polar` ramp and the chart hexes are converted from their sRGB.
mod polar {
    type Lch = (f32, f32, f32);

    pub const WHITE: Lch = (1.0, 0.0, 0.0);
    pub const BLACK: Lch = (0.0, 0.0, 0.0);

    // Light-mode neutrals (cool gray ramp).
    pub const GRAY_50: Lch = (0.985, 0.002, 247.839);
    pub const GRAY_100: Lch = (0.967, 0.003, 264.542);
    pub const GRAY_200: Lch = (0.928, 0.006, 264.531);
    pub const GRAY_300: Lch = (0.872, 0.01, 258.338);
    pub const GRAY_400: Lch = (0.707, 0.022, 261.325);
    pub const GRAY_500: Lch = (0.551, 0.027, 264.364);
    pub const GRAY_600: Lch = (0.446, 0.03, 256.802);
    pub const GRAY_900: Lch = (0.21, 0.034, 264.665);

    // Dark-mode neutrals: hsl(0, 0%, 85% … 3.5%).
    pub const POLAR_50: Lch = (0.8845, 0.0, 0.0);
    pub const POLAR_100: Lch = (0.8373, 0.0, 0.0);
    // hsl(0, 0%, 63%): muted text, a step brighter than 400.
    pub const POLAR_300: Lch = (0.7090, 0.0, 0.0);
    pub const POLAR_400: Lch = (0.6154, 0.0, 0.0);
    pub const POLAR_500: Lch = (0.5634, 0.0, 0.0);
    pub const POLAR_600: Lch = (0.2801, 0.0, 0.0);
    pub const POLAR_700: Lch = (0.2484, 0.0, 0.0);
    pub const POLAR_800: Lch = (0.2100, 0.0, 0.0);
    pub const POLAR_900: Lch = (0.1757, 0.0, 0.0);
    pub const POLAR_950: Lch = (0.1394, 0.0, 0.0);

    // Emerald: success.
    pub const EMERALD_50: Lch = (0.979, 0.021, 166.113);
    pub const EMERALD_300: Lch = (0.845, 0.143, 164.978);
    pub const EMERALD_400: Lch = (0.765, 0.177, 163.223);
    pub const EMERALD_500: Lch = (0.696, 0.17, 162.48);
    pub const EMERALD_600: Lch = (0.596, 0.145, 163.225);
    pub const EMERALD_700: Lch = (0.508, 0.118, 165.612);
    pub const EMERALD_950: Lch = (0.262, 0.051, 172.552);

    // Red: danger.
    pub const RED_50: Lch = (0.971, 0.013, 17.38);
    pub const RED_300: Lch = (0.808, 0.114, 19.571);
    pub const RED_400: Lch = (0.704, 0.191, 22.216);
    pub const RED_500: Lch = (0.637, 0.237, 25.331);
    pub const RED_600: Lch = (0.577, 0.245, 27.325);
    pub const RED_700: Lch = (0.505, 0.213, 27.518);
    pub const RED_950: Lch = (0.258, 0.092, 26.042);

    // Amber: warning.
    pub const AMBER_50: Lch = (0.987, 0.022, 95.277);
    pub const AMBER_300: Lch = (0.879, 0.169, 91.605);
    pub const AMBER_400: Lch = (0.828, 0.189, 84.429);
    pub const AMBER_500: Lch = (0.769, 0.188, 70.08);
    pub const AMBER_600: Lch = (0.666, 0.179, 58.318);
    pub const AMBER_700: Lch = (0.555, 0.163, 48.998);
    pub const AMBER_950: Lch = (0.279, 0.077, 45.635);

    // Indigo: accent, brand and info.
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

    // Blue: links, focus and the blue pill.
    pub const BLUE_100: Lch = (0.932, 0.032, 255.585);
    pub const BLUE_200: Lch = (0.882, 0.059, 254.128);
    pub const BLUE_300: Lch = (0.809, 0.105, 251.813);
    pub const BLUE_400: Lch = (0.707, 0.165, 254.624);
    pub const BLUE_500: Lch = (0.623, 0.214, 259.815);
    pub const BLUE_600: Lch = (0.546, 0.245, 262.881);
    pub const BLUE_700: Lch = (0.488, 0.243, 264.376);
    pub const BLUE_900: Lch = (0.379, 0.146, 265.522);

    // Purple: the purple pill.
    pub const PURPLE_300: Lch = (0.827, 0.119, 306.383);
    pub const PURPLE_400: Lch = (0.714, 0.203, 305.504);
    pub const PURPLE_600: Lch = (0.558, 0.288, 302.321);
    pub const PURPLE_700: Lch = (0.496, 0.265, 301.924);

    // Cyan: Tailwind's defaults, which Polar inherits but never overrides.
    pub const CYAN_300: Lch = (0.865, 0.127, 207.078);
    pub const CYAN_400: Lch = (0.789, 0.154, 211.53);
    pub const CYAN_600: Lch = (0.609, 0.126, 221.723);
    pub const CYAN_700: Lch = (0.52, 0.105, 223.128);

    /// Claude's clay, #D97757: a brand colour, the same in both modes.
    pub const CLAUDE_CLAY: Lch = (0.6724, 0.1308, 38.76);

    /// The dashboard's categorical chart colours (cancellation reasons):
    /// #EF765F, #F3AB71, #F0C289, #8ED6B9, #618BE6, #2A459D, #EF8EB0, #CEC7C7.
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

/// The largest chroma up to `c` that sRGB shows at this lightness and hue.
/// Tailwind's ramps reach into Display P3; a browser on an sRGB screen maps
/// them back the same way rather than clipping channels.
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
    /// Polar's tokens in one mode.
    ///
    /// Where Orbit names the role (backgrounds,
    /// text, borders, status) its semantic token is used as is; where only a
    /// component shows it (hover, selected, focus, primary button) that
    /// component's classes; and where Polar has nothing (pressed, terminal,
    /// syntax, selection) the nearest Polar colour with the same intent.
    #[expect(
        clippy::too_many_lines,
        reason = "one flat table of every colour token reads best in one place"
    )]
    fn build(mode: ThemeMode) -> Self {
        use polar::{
            AMBER_50, AMBER_300, AMBER_400, AMBER_500, AMBER_600, AMBER_700, AMBER_950, BLACK,
            BLUE_100, BLUE_200, BLUE_300, BLUE_400, BLUE_500, BLUE_600, BLUE_700, BLUE_900, CHART,
            CLAUDE_CLAY, CYAN_300, CYAN_400, CYAN_600, CYAN_700, EMERALD_50, EMERALD_300,
            EMERALD_400, EMERALD_500, EMERALD_600, EMERALD_700, EMERALD_950, GRAY_50, GRAY_100,
            GRAY_200, GRAY_300, GRAY_400, GRAY_500, GRAY_600, GRAY_900, INDIGO_50, INDIGO_100,
            INDIGO_200, INDIGO_300, INDIGO_400, INDIGO_500, INDIGO_600, INDIGO_700, INDIGO_800,
            INDIGO_900, INDIGO_950, POLAR_50, POLAR_100, POLAR_300, POLAR_400, POLAR_500,
            POLAR_600, POLAR_700, POLAR_800, POLAR_900, POLAR_950, PURPLE_300, PURPLE_400,
            PURPLE_600, PURPLE_700, RED_50, RED_300, RED_400, RED_500, RED_600, RED_700, RED_950,
            WHITE,
        };
        let dark = mode.is_dark();
        let pick = |light: (f32, f32, f32), dark_value: (f32, f32, f32)| {
            token(if dark { dark_value } else { light })
        };
        let ramp = |light: [(f32, f32, f32); 12], dark_value: [(f32, f32, f32); 12]| {
            Scale(if dark { dark_value } else { light }.map(token))
        };
        // Radix-style steps over Polar's ramps: 1–2 backgrounds, 3–5 fills,
        // 6–8 borders, 9–10 solids, 11 secondary text, 12 primary text.
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
        // Indigo is Orbit's accent: background-accent at step 3, text-accent
        // at step 11.
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
        // Orbit's background-*, text-* and (by border-warning's pattern)
        // 300/700 borders; solids follow the destructive button, and amber
        // and emerald fills take their 950 ink, since white doesn't read on them.
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
        // Orbit's link button.
        let link = token(BLUE_400);
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
            // The modal backdrop: bg-black/70.
            scrim: black.with_alpha(0.7).resolved(),
            // Orbit's shadow ink: rgba(0 0 0 / 0.06).
            shadow: black.with_alpha(0.06).resolved(),
            palette: CHART.map(token),
            claude: token(CLAUDE_CLAY),
            ansi: AnsiColors(ansi.map(token)),
            // The dashboard: a gray-100 (dark: sidebar polar-950) frame around
            // a white (dark: polar-900) content card.
            frame: pick(GRAY_100, POLAR_950),
            background: pick(WHITE, POLAR_900),
            // Lists, segmented controls and fields on the card.
            surface: pick(GRAY_50, POLAR_800),
            // Secondary buttons at rest and menus.
            raised: pick(GRAY_100, POLAR_800),
            // Ghost and outline buttons under the pointer.
            hover: pick(GRAY_200, POLAR_700),
            // The active tab.
            selected: pick(GRAY_100, POLAR_700),
            // A secondary button under the pointer, the deepest fill Polar has.
            pressed: pick(GRAY_200, POLAR_600),
            // border-primary.
            line: pick(GRAY_200, POLAR_700),
            // Sidebar borders and checkbox edges.
            line_strong: pick(GRAY_300, POLAR_600),
            // text-primary, text-secondary, text-tertiary.
            text: pick(BLACK, WHITE),
            text_muted: pick(GRAY_500, POLAR_300),
            text_faint,
            // A focused input: blue-300 (dark: blue-700) around it.
            ring: pick(BLUE_300, BLUE_700),
            // The blue pill's tint behind text.
            text_selection: pick(BLUE_100, BLUE_900),
            // The default button: black on light, white on dark; it fades to
            // 85% under the pointer, the polar step that blend lands on.
            primary: pick(BLACK, WHITE),
            primary_hover: pick(POLAR_600, POLAR_50),
            primary_text: pick(WHITE, BLACK),
            action_top: token(BLUE_500),
            action_bottom: token(BLUE_700),
            action_hover_top: token(BLUE_400),
            action_hover_bottom: token(BLUE_600),
            action_text: token(WHITE),
            destructive: red.solid,
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
            question_tint: pick(INDIGO_50, INDIGO_950),
            thread_title_active: link,
            mention_skill: link,
            mention_connector: pick(GRAY_500, POLAR_300),
            mention_command: amber.text,
            // Polar's pills' text colours, one hue per role.
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

/// Font families and text sizes shared by application components.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypographyTokens {
    /// The set the families below come from.
    pub fonts: FontSet,
    /// `--font-sans`: `"Spline Sans"`, weights 300–700.
    pub sans: FontRole,
    /// `--font-mono`: `"Spline Sans Mono"`, weights 300–700.
    pub mono: FontRole,
    /// `--font-heading`: `"Spline Sans"` for headings.
    pub heading: FontRole,
    /// Sans tracking in em: [`ProseTypography::TRACKING_EM`] for Spline,
    /// which is drawn loose and tuned with it; none for system fonts.
    pub tracking_em: f32,
    /// Control text, 14 px.
    pub control_text: Pixels,
    /// Editor text, 14 px.
    pub editor_text_desktop: Pixels,
    /// Labels, group headings, shortcuts: `text-xs`, 12 px.
    pub label_text: Pixels,
}

/// The text-size ramp: 10, 11, 12, 12.5, 13, 14, 15, 16, 18, 20 and 22 px.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum TextStep {
    /// 10 px: glass badges and code annotations.
    X3s,
    /// 11 px.
    X2s,
    /// `text-xs`, 12 px: labels, captions and tooltips.
    Xs,
    /// 12.5 px: terminal output.
    Terminal,
    /// 13 px: compact chrome (tabs, card captions).
    XsMid,
    /// `text-sm`, 14 px: controls and rows.
    Sm,
    /// 15 px.
    SmMid,
    /// `text-base`, 16 px.
    Base,
    /// `text-lg`, 18 px.
    Lg,
    /// `text-xl`, 20 px: page titles.
    Xl,
    /// 22 px: stat values.
    X2l,
}

impl TypographyTokens {
    /// The exact pixel size of a text step.
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

/// Which families the Editor draws text in: its bundled Spline faces or
/// the platform's own UI and monospace fonts.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FontSet {
    /// Spline Sans and Spline Sans Mono.
    #[default]
    Spline,
    /// The platform's UI font and its monospace font.
    System,
}

impl FontSet {
    pub const ALL: [Self; 2] = [Self::System, Self::Spline];

    /// The value stored in `settings.json`.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Spline => "spline",
            Self::System => "system",
        }
    }

    /// The set stored as `key`; anything else reads as the default.
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

/// A font family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FontRole {
    /// Family name.
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
    /// Spline's tracking: −0.04 em. The window root sets
    /// [`TypographyTokens::tracking_em`] once with GPUI's `tracking`, so
    /// every sans text inherits it at its own size; monospaced code opts out
    /// with `tracking(0.0)`.
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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
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

/// Shared corner radii: the 4–26 px ramp, with the hairline steps below it
/// and the half steps between its larger values.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RadiusStep {
    /// 1 px: chart bars and hairline marks.
    X3s,
    /// 2 px: thin bars and carets.
    X2s,
    /// 3 px: small image marks.
    X2sMid,
    /// `--radius-xs` — 4 px.
    Xs,
    /// `--radius-sm` — 6 px.
    Sm,
    /// `--radius-md` — 8 px.
    Md,
    /// `--radius-lg` — 10 px.
    Lg,
    /// 12 px: menu rows.
    LgMid,
    /// `--radius-xl` — 14 px.
    Xl,
    /// 16 px.
    XlMid,
    /// `--radius-2xl` — 18 px.
    X2l,
    /// 20 px.
    X2lMid,
    /// `--radius-3xl` — 22 px.
    X3l,
    /// `--radius-4xl` — 26 px.
    X4l,
}

/// Marker holding ramp arithmetic shared across call sites.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RadiusTokens;

impl RadiusTokens {
    /// `--radius` itself: 0.625rem at the 16 px root size.
    pub const BASE_PX: f32 = 10.0;

    /// The exact pixel value of a ramp step.
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

    /// Subtracts the inset gap from an outer radius, clamping at zero.
    #[must_use]
    pub fn nested(surface: Pixels, gap: Pixels) -> Pixels {
        let inner = f32::from(surface) - f32::from(gap);
        px(inner.max(0.0))
    }
}

/// One outer shadow layer, written the way CSS writes `box-shadow`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowLayer {
    /// The shadow's ink at its strength.
    pub color: Oklch,
    /// X offset (positive rightward).
    pub offset_x: Pixels,
    /// Y offset (positive downward).
    pub offset_y: Pixels,
    /// CSS blur radius: the shadow fades over this distance either side of
    /// its edge.
    pub blur_radius: Pixels,
    /// Spread radius (negative tucks the shadow under the element).
    pub spread_radius: Pixels,
}

impl ShadowLayer {
    /// The GPUI outer shadow. GPUI takes the Gaussian's sigma as its blur
    /// radius, where CSS takes twice the sigma, so a CSS blur is halved:
    /// passed through unchanged, every shadow spreads twice as far.
    #[must_use]
    pub fn to_box_shadow(self) -> gpui::BoxShadow {
        gpui::BoxShadow {
            color: self.color.to_paint(),
            offset: gpui::Point {
                x: self.offset_x,
                y: self.offset_y,
            },
            blur_radius: self.blur_radius / 2.0,
            spread_radius: self.spread_radius,
            inset: false,
        }
    }
}

/// The shadows, one stack per height above the canvas.
///
/// Each stack is a
/// hairline that draws the edge plus layered drops whose offset is half
/// their blur and whose negative spread keeps them tucked under the
/// element, so light falls from above and nothing haloes the sides. Light
/// mode inks them faintly; dark mode needs far more to show at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElevationTokens {
    /// The half-pixel ring that draws every elevated edge.
    pub hairline: ShadowLayer,
    /// Controls and small cards resting on a surface: select triggers,
    /// attachments, chat bubbles, code cards, settings cards. Its first
    /// layer is a white top sheen.
    pub raised: [ShadowLayer; 3],
    /// Glass cards floating over the canvas: the composer.
    pub floating: [ShadowLayer; 3],
    /// Menus, popovers, hover cards and dialogs over everything else.
    pub overlay: [ShadowLayer; 4],
}

impl ElevationTokens {
    /// GPUI shadows for some layers.
    #[must_use]
    pub fn box_shadows(layers: &[ShadowLayer]) -> Vec<gpui::BoxShadow> {
        layers.iter().map(|layer| layer.to_box_shadow()).collect()
    }

    /// The drop layers of one height, without the hairline: for a surface
    /// that draws its own edge.
    #[must_use]
    pub const fn drops(&self, elevation: Elevation) -> &[ShadowLayer] {
        match elevation {
            Elevation::Raised => &self.raised,
            Elevation::Floating => &self.floating,
            Elevation::Overlay => &self.overlay,
        }
    }

    /// The GPUI shadows of one height: the hairline, then its drops.
    #[must_use]
    pub fn shadows(&self, elevation: Elevation) -> Vec<gpui::BoxShadow> {
        let mut shadows = vec![self.hairline.to_box_shadow()];
        shadows.extend(Self::box_shadows(self.drops(elevation)));
        shadows
    }

    fn build(mode: ThemeMode, colors: &ColorTokens) -> Self {
        let shadow = |color: Oklch, strength: f32, oy: f32, blur: f32, spread: f32| ShadowLayer {
            color: color.with_alpha(strength).resolved(),
            offset_x: px(0.0),
            offset_y: px(oy),
            blur_radius: px(blur),
            spread_radius: px(spread),
        };
        // Light mode's ink is the darkest gray, carrying the palette's hue,
        // so shadows read as shade rather than grey smudge.
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

/// How far a surface sits above the canvas; picks its shadow stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Elevation {
    /// Resting on a surface.
    Raised,
    /// Floating over the canvas.
    Floating,
    /// Over everything else.
    Overlay,
}

/// Shared component dimensions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Focus ring width; its colour is [`ColorTokens::ring`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InteractionTokens {
    /// Focus-ring width, 3 px.
    pub focus_ring_width: Pixels,
}

/// The complete Artisan theme, composed from typed token groups. The app
/// installs it as a GPUI global ([`install`], [`set_mode`]); everything reads
/// it through [`ActiveTheme::theme`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ArtisanTheme {
    /// Colour tokens of the active mode.
    pub colors: ColorTokens,
    /// Font families and text sizes.
    pub typography: TypographyTokens,
    /// The 4 px spacing unit.
    pub spacing: SpacingTokens,
    /// The corner ramp and nested-radius arithmetic.
    pub radius: RadiusTokens,
    /// Shadows.
    pub elevation: ElevationTokens,
    /// Shared control sizes.
    pub density: DensityTokens,
    /// Focus treatment.
    pub interaction: InteractionTokens,
}

impl gpui::Global for ArtisanTheme {}

impl TypographyTokens {
    /// The tokens with `fonts`' families.
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

    /// `heading`'s tracking in pixels; Spline's only, like [`Self::tracking_em`].
    #[must_use]
    pub const fn heading_tracking_px(&self, heading: &ProseHeading) -> f32 {
        match self.fonts {
            FontSet::Spline => heading.tracking_px,
            FontSet::System => 0.0,
        }
    }
}

/// Installs the application theme. The app calls this once at startup.
pub fn install(mode: ThemeMode, cx: &mut gpui::App) {
    cx.set_global(*ArtisanTheme::for_mode(mode));
}

/// Switches every window to `mode`, keeping the font set. Cached views
/// re-render because the windows refresh.
pub fn set_mode(mode: ThemeMode, cx: &mut gpui::App) {
    if cx.theme().colors.mode == mode {
        return;
    }
    let typography = cx.theme().typography;
    cx.set_global(ArtisanTheme {
        typography,
        ..*ArtisanTheme::for_mode(mode)
    });
    cx.refresh_windows();
}

/// Switches every window to `fonts`' families.
pub fn set_fonts(fonts: FontSet, cx: &mut gpui::App) {
    if cx.theme().typography.fonts == fonts {
        return;
    }
    let theme = ArtisanTheme {
        typography: TypographyTokens::new(fonts),
        ..*cx.theme()
    };
    cx.set_global(theme);
    cx.refresh_windows();
}

/// Reads the installed application theme.
pub trait ActiveTheme {
    /// The application theme.
    fn theme(&self) -> &ArtisanTheme;
}

impl ActiveTheme for gpui::App {
    /// The installed theme. A context nothing installed into (a component
    /// test) reads the dark theme.
    fn theme(&self) -> &ArtisanTheme {
        self.try_global::<ArtisanTheme>()
            .unwrap_or_else(|| ArtisanTheme::for_mode(ThemeMode::Dark))
    }
}

impl ArtisanTheme {
    /// The dark values. Production code reads the installed global through
    /// [`ActiveTheme::theme`], which follows the user's mode.
    #[must_use]
    pub fn resolved() -> Self {
        *Self::for_mode(ThemeMode::Dark)
    }

    /// The theme in one mode, built once.
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

/// How long the diagonal sweep between modes takes.
const MODE_TRANSITION: std::time::Duration = std::time::Duration::from_millis(700);

/// Starts the diagonal shimmer that carries the window into `next`.
///
/// The current mode's last frame sweeps away from the top-left corner to
/// the bottom-right behind a band of `next`'s link colour. Call it right
/// before switching, in the same update; with reduced motion the mode simply
/// changes.
pub fn begin_mode_transition(next: ThemeMode, window: &mut gpui::Window, cx: &gpui::App) {
    if cx.reduce_motion() {
        return;
    }
    window.begin_snapshot_transition(gpui::SnapshotTransition {
        duration: MODE_TRANSITION,
        band: 0.18,
        shimmer: ArtisanTheme::for_mode(next)
            .colors
            .link
            .fade(0.45)
            .to_paint(),
    });
}

#[cfg(test)]
mod polar_tokens {
    use super::*;

    /// Both modes keep text readable: body text far above the card,
    /// secondary text dimmed but still readable on cards too, and the
    /// primary button's label on its fill.
    #[test]
    fn text_reads_in_both_modes() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let colors = &ArtisanTheme::for_mode(mode).colors;
            assert_eq!(colors.mode, mode);
            for (role, color, floor) in [
                ("text", colors.text, 12.0),
                ("secondary text", colors.text_muted, 4.5),
            ] {
                for (face, behind) in [("card", colors.background), ("surface", colors.surface)] {
                    let ratio = contrast(color, behind);
                    assert!(
                        ratio >= floor,
                        "{mode:?}: {role} on {face} contrast {ratio:.2}"
                    );
                }
            }
            let label = contrast(colors.primary_text, colors.primary);
            assert!(
                label >= 4.0,
                "{mode:?}: primary button label contrast {label:.2}"
            );
        }
    }

    /// WCAG 2 contrast ratio of two painted colours.
    fn contrast(a: Oklch, b: Oklch) -> f32 {
        let luminance = |color: Oklch| {
            let rgb = color.to_srgb();
            let linear = |channel: f32| {
                if channel <= 0.040_45 {
                    channel / 12.92
                } else {
                    ((channel + 0.055) / 1.055).powf(2.4)
                }
            };
            0.0722f32.mul_add(
                linear(rgb.b),
                0.7152f32.mul_add(linear(rgb.g), 0.2126 * linear(rgb.r)),
            )
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
}
