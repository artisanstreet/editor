//! Shared glass materials, highlights, blur and shadows.

#![forbid(unsafe_code)]

use crate::theme::{
    ArtisanTheme, Elevation, ElevationTokens, Oklch, Opacity, RadiusStep, RadiusTokens, TextStep,
    ThemeMode, TypographyTokens,
};
use gpui::{
    AnyElement, App, Background, BoxShadow, Div, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _, Window, div,
    linear_color_stop, linear_gradient, point, px,
};

/// Shared glass treatments for the composer and floating controls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlassStrength {
    /// The quiet treatment used by the composer surface.
    Quiet,
    /// The stronger treatment used by picker and tooltip surfaces.
    Strong,
}

/// Backdrop blur for the composer.
const GLASS_QUIET_BLUR_RADIUS_PX: f32 = 12.0;
/// Backdrop blur for floating controls.
const GLASS_STRONG_BLUR_RADIUS_PX: f32 = 20.0;

/// Resolves the material's backdrop blur radius.
#[must_use]
pub const fn glass_blur_radius(strength: GlassStrength) -> Pixels {
    px(match strength {
        GlassStrength::Quiet => GLASS_QUIET_BLUR_RADIUS_PX,
        GlassStrength::Strong => GLASS_STRONG_BLUR_RADIUS_PX,
    })
}

/// Translucent diagonal face: gray 7 at the near corner to gray 4 at the
/// far one, over content the app doesn't control.
#[must_use]
pub fn glass_material(theme: &ArtisanTheme, strength: GlassStrength) -> Background {
    let (from, to) = match strength {
        GlassStrength::Quiet => (Opacity::GlassFaceQuiet, Opacity::GlassFaceQuietFar),
        GlassStrength::Strong => (Opacity::GlassFaceStrong, Opacity::GlassFaceStrongFar),
    };
    let gray = &theme.colors.gray;
    linear_gradient(
        145.0,
        linear_color_stop(gray.step(7).at(from).to_paint(), 0.0),
        linear_color_stop(gray.step(4).at(to).to_paint(), 1.0),
    )
}

/// A top sheen of `colors.white` at `purpose`, fading to clear at `end`.
fn sheen(theme: &ArtisanTheme, purpose: Opacity, end: f32) -> Background {
    let white: Oklch = theme.colors.white;
    linear_gradient(
        180.0,
        linear_color_stop(white.at(purpose).to_paint(), 0.0),
        linear_color_stop(white.at(Opacity::Clear).to_paint(), end),
    )
}

/// Top highlight fading to transparency before the middle of the material.
#[must_use]
pub fn glass_highlight(theme: &ArtisanTheme, strength: GlassStrength) -> Background {
    match strength {
        GlassStrength::Quiet => sheen(theme, Opacity::Sheen, 0.42),
        GlassStrength::Strong => sheen(theme, Opacity::SheenStrong, 0.44),
    }
}

/// Adds the visual-only material without installing any pointer handlers.
#[must_use]
pub fn glass_material_layer(theme: &ArtisanTheme, strength: GlassStrength, radius: Pixels) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .bg(glass_material(theme, strength))
}

/// Adds the highlight without installing pointer handlers.
#[must_use]
pub fn glass_highlight_layer(theme: &ArtisanTheme, strength: GlassStrength, radius: Pixels) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .bg(glass_highlight(theme, strength))
}

/// Translucent text-coloured base beneath the glass gradient.
#[must_use]
pub fn glass_foreground_base(theme: &ArtisanTheme) -> Hsla {
    theme.colors.text.at(Opacity::GlassBase).to_paint()
}

/// The fill of a control resting on glass (picker tabs, selects).
///
/// A solid well, darkest at the foot, so it sits quietly below the glass
/// instead of lifting off it. Dark mode runs hover grey down to the content card's
/// near-black; light mode runs the card's white down to the raised grey.
#[must_use]
pub fn glass_control_fill(theme: &ArtisanTheme) -> Background {
    let colors = &theme.colors;
    let (top, bottom) = match colors.mode {
        ThemeMode::Dark => (colors.hover, colors.background),
        ThemeMode::Light => (colors.background, colors.raised),
    };
    linear_gradient(
        180.0,
        linear_color_stop(top.to_paint(), 0.0),
        linear_color_stop(bottom.to_paint(), 1.0),
    )
}

/// The colour of the rule between groups in every glass menu: a hairline
/// of the text colour, translucent so it reads on any backdrop.
#[must_use]
pub fn glass_menu_line(theme: &ArtisanTheme) -> Hsla {
    theme.colors.text.at(Opacity::Hairline).to_paint()
}

/// The rule between groups in every glass menu: one pixel of
/// [`glass_menu_line`], 4 px clear above and below, the menu's full width.
#[must_use]
pub fn glass_menu_separator(theme: &ArtisanTheme) -> Div {
    div()
        .flex_shrink_0()
        .h(px(1.0))
        .my(px(4.0))
        .bg(glass_menu_line(theme))
}

/// The raised shadow stack of controls and small cards resting on a
/// surface. Glass rims belong to [`glass_card_shadows`].
#[must_use]
pub fn card_shadows(theme: &ArtisanTheme) -> Vec<BoxShadow> {
    theme.elevation.shadows(Elevation::Raised)
}

/// [`card_shadows`] in a tone instead of a border.
///
/// A soft half-pixel ring and a faint glow of the tone under the raised
/// drops, for small tinted cards and pills (a blocked goal's state) whose
/// solid border reads harsh.
#[must_use]
pub fn tinted_card_shadows(theme: &ArtisanTheme, tone: Oklch) -> Vec<BoxShadow> {
    let mut shadows = vec![
        glass_shadow(tone.with_alpha(0.35), 0.0, 0.0, 0.5, false),
        BoxShadow {
            color: tone.with_alpha(0.18).to_paint(),
            offset: point(px(0.0), px(1.0)),
            blur_radius: px(3.0),
            spread_radius: px(0.0),
            inset: false,
        },
    ];
    shadows.extend(ElevationTokens::box_shadows(
        theme.elevation.drops(Elevation::Raised),
    ));
    shadows
}

/// A glass card's rims and the shadow stack of its elevation. These do not
/// add a background fill.
#[must_use]
pub fn glass_card_shadows(theme: &ArtisanTheme, elevation: Elevation) -> Vec<BoxShadow> {
    let mut shadows = glass_rims(theme);
    shadows.extend(theme.elevation.shadows(elevation));
    shadows
}

/// The rims alone, for glass nested inside other glass, which casts no
/// shadow of its own: the bright outer top rim, the inner top light and
/// the inner bottom shade. The shade is a whisper on light glass.
#[must_use]
pub fn glass_rims(theme: &ArtisanTheme) -> Vec<BoxShadow> {
    let colors = &theme.colors;
    let shade = match colors.mode {
        ThemeMode::Light => colors.black.with_alpha(0.05),
        ThemeMode::Dark => colors.black.at(Opacity::RimShade),
    };
    vec![
        glass_shadow(colors.white.at(Opacity::RimLight), -1.0, 0.0, 0.0, false),
        glass_shadow(colors.white.at(Opacity::RimInner), 1.0, 0.0, 0.0, true),
        glass_shadow(shade, -1.0, 0.0, 0.0, true),
    ]
}

fn glass_shadow(
    color: Oklch,
    offset_y: f32,
    blur_radius: f32,
    spread_radius: f32,
    inset: bool,
) -> BoxShadow {
    BoxShadow {
        color: color.to_paint(),
        offset: point(px(0.0), px(offset_y)),
        blur_radius: px(blur_radius),
        spread_radius: px(spread_radius),
        inset,
    }
}

/// Short text in a small quiet-glass pill: a group's row count, the build id.
#[must_use]
pub fn glass_badge(theme: &ArtisanTheme, text: impl Into<SharedString>) -> GlassBadge {
    GlassBadge {
        theme: *theme,
        text: text.into(),
        selector: None,
    }
}
#[derive(IntoElement)]
pub struct GlassBadge {
    theme: ArtisanTheme,
    text: SharedString,
    selector: Option<SharedString>,
}
impl GlassBadge {
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
}
impl RenderOnce for GlassBadge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        static FEATURES: std::sync::LazyLock<gpui::FontFeatures> = std::sync::LazyLock::new(|| {
            gpui::FontFeatures(std::sync::Arc::new(vec![("tnum".into(), 1)]))
        });
        let radius = px(8.0);
        let mut root = div()
            .relative()
            .flex_none()
            .h(px(16.0))
            .min_w(px(16.0))
            .px(px(5.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded(radius)
            .overflow_hidden()
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&self.theme))
            .child(glass_material_layer(
                &self.theme,
                GlassStrength::Quiet,
                radius,
            ))
            .child(glass_highlight_layer(
                &self.theme,
                GlassStrength::Quiet,
                radius,
            ))
            .child(
                div()
                    .relative()
                    .text_size(TypographyTokens::size(TextStep::X3s))
                    .line_height(px(16.0))
                    .font_features(FEATURES.clone())
                    .text_color(self.theme.colors.text.to_paint())
                    .child(self.text),
            );
        if let Some(selector) = self.selector {
            root = root.debug_selector(move || selector.to_string());
        }
        root
    }
}

pub(crate) const COMPOSER_RADIUS: Pixels = RadiusTokens::value(RadiusStep::X2l);
/// Draft chrome shares geometry with its optional TabCard shell. Content and
/// input/drop handlers remain owned by the draft view.
#[derive(IntoElement)]
pub struct ComposerFrame {
    theme: ArtisanTheme,
    content: AnyElement,
    selector: Option<gpui::SharedString>,
    nested: bool,
}
impl ComposerFrame {
    pub fn new(theme: ArtisanTheme, content: impl IntoElement) -> Self {
        Self {
            theme,
            content: content.into_any_element(),
            selector: None,
            nested: false,
        }
    }
    /// Sits inside another glass shell that casts the shadow, so the frame
    /// keeps only its rims.
    #[must_use]
    pub const fn nested(mut self, nested: bool) -> Self {
        self.nested = nested;
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<gpui::SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
}
impl RenderOnce for ComposerFrame {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let radius = COMPOSER_RADIUS;
        let mut root = div()
            .relative()
            .w_full()
            .flex()
            .flex_col()
            .min_h(px(112.0))
            .p(px(8.0))
            .rounded(radius)
            .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
            .bg(glass_foreground_base(&self.theme))
            .shadow(if self.nested {
                glass_rims(&self.theme)
            } else {
                glass_card_shadows(&self.theme, Elevation::Floating)
            })
            .child(glass_material_layer(
                &self.theme,
                GlassStrength::Quiet,
                radius,
            ))
            .child(glass_highlight_layer(
                &self.theme,
                GlassStrength::Quiet,
                radius,
            ))
            .child(self.content);
        if let Some(selector) = self.selector {
            root = root.debug_selector(move || selector.to_string());
        }
        root
    }
}

/// Materials for the shared tabbed card; geometry lives in `TabCard`.
pub(crate) struct TabCardMaterial {
    pub blur: Pixels,
    pub shell: Background,
    pub strip: Background,
    pub highlight: Background,
    pub well: Hsla,
    pub ring: Hsla,
    pub top_light: Background,
    pub shadow: Vec<BoxShadow>,
}
impl TabCardMaterial {
    pub fn new(theme: &ArtisanTheme) -> Self {
        let colors = &theme.colors;
        let gradient = |a: Oklch, b: Oklch| {
            linear_gradient(
                145.0,
                linear_color_stop(a.to_paint(), 0.0),
                linear_color_stop(b.to_paint(), 1.0),
            )
        };
        let pane = |step: usize| colors.gray.step(step).at(Opacity::GlassPane);
        Self {
            blur: glass_blur_radius(GlassStrength::Strong),
            shell: gradient(pane(4), pane(2)),
            // Lighter than the shell, from its lit corner down to where the
            // shell's own gradient starts, as the S700 to S800 strip did.
            strip: gradient(colors.gray.step(8).at(Opacity::GlassPaneLit), pane(7)),
            highlight: sheen(theme, Opacity::SheenFaint, 0.35),
            well: colors.background.to_paint(),
            ring: colors.text.at(Opacity::HairlineFaint).to_paint(),
            top_light: sheen(theme, Opacity::SheenSoft, 0.4),
            // The shell strokes its own edge.
            shadow: ElevationTokens::box_shadows(theme.elevation.drops(Elevation::Floating)),
        }
    }
}

#[cfg(test)]
mod composer_frame_tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Draft {
        loaded: bool,
    }
    impl Render for Draft {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().flex_col().child(
                ComposerFrame::new(
                    ArtisanTheme::resolved(),
                    div()
                        .h(px(96.0))
                        .child(if self.loaded { "Ready" } else { "Loading" }),
                )
                .debug_selector("composer-frame-test"),
            )
        }
    }
    #[gpui::test]
    fn draft_frame_size_is_independent_of_async_content_state(cx: &mut TestAppContext) {
        let (draft, cx) = cx.add_window_view(|_, _| Draft { loaded: false });
        cx.simulate_resize(gpui::size(px(480.0), px(400.0)));
        let before = cx.debug_bounds("composer-frame-test").unwrap();
        assert_eq!(before.size, gpui::size(px(480.0), px(112.0)));
        draft.update(cx, |draft, cx| {
            draft.loaded = true;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("composer-frame-test").unwrap(), before);
        cx.simulate_resize(gpui::size(px(320.0), px(400.0)));
        assert_eq!(
            cx.debug_bounds("composer-frame-test").unwrap().size,
            gpui::size(px(320.0), px(112.0))
        );
    }
}
