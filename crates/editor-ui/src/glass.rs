//! Shared glass materials, highlights, blur and shadows.

#![forbid(unsafe_code)]

use crate::theme::{
    ArtisanTheme, Oklch, RadiusStep, RadiusTokens, ShadowLayer, SurfaceStep, TextStep,
    TypographyTokens,
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
pub fn glass_blur_radius(strength: GlassStrength) -> Pixels {
    px(match strength {
        GlassStrength::Quiet => GLASS_QUIET_BLUR_RADIUS_PX,
        GlassStrength::Strong => GLASS_STRONG_BLUR_RADIUS_PX,
    })
}

/// Translucent diagonal face using the shared neutral ramp.
#[must_use]
pub fn glass_material(strength: GlassStrength) -> Background {
    let (from_alpha, to_alpha) = match strength {
        GlassStrength::Quiet => (0.20, 0.14),
        GlassStrength::Strong => (0.28, 0.20),
    };
    linear_gradient(
        145.0,
        linear_color_stop(
            SurfaceStep::S600.oklch().with_alpha(from_alpha).to_paint(),
            0.0,
        ),
        linear_color_stop(
            SurfaceStep::S800.oklch().with_alpha(to_alpha).to_paint(),
            1.0,
        ),
    )
}

/// Top highlight fading to transparency before the middle of the material.
#[must_use]
pub fn glass_highlight(strength: GlassStrength) -> Background {
    let (alpha, transparent_at) = match strength {
        GlassStrength::Quiet => (0.05, 0.42),
        GlassStrength::Strong => (0.08, 0.44),
    };
    let white = Oklch::new(1.0, 0.0, 0.0);
    linear_gradient(
        180.0,
        linear_color_stop(white.with_alpha(alpha).to_paint(), 0.0),
        linear_color_stop(white.with_alpha(0.0).to_paint(), transparent_at),
    )
}

/// Adds the visual-only material without installing any pointer handlers.
#[must_use]
pub fn glass_material_layer(strength: GlassStrength, radius: Pixels) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .bg(glass_material(strength))
}

/// Adds the highlight without installing pointer handlers.
#[must_use]
pub fn glass_highlight_layer(strength: GlassStrength, radius: Pixels) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .bg(glass_highlight(strength))
}

/// Translucent foreground base beneath the glass gradient.
#[must_use]
pub fn glass_foreground_base(theme: &ArtisanTheme) -> Hsla {
    theme.colors.foreground.with_alpha(0.05).to_paint()
}

/// Card shadow stack. Glass rims belong to [`glass_card_shadows`].
#[must_use]
pub fn card_shadows(theme: &ArtisanTheme) -> Vec<BoxShadow> {
    theme
        .elevation
        .card_shadow
        .iter()
        .copied()
        .map(ShadowLayer::to_box_shadow)
        .collect()
}

/// Glass rim and elevation shadows. These do not add a background fill.
#[must_use]
pub fn glass_card_shadows() -> Vec<BoxShadow> {
    let white = Oklch::new(1.0, 0.0, 0.0);
    let black = Oklch::new(0.0, 0.0, 0.0);
    vec![
        glass_shadow(white.with_alpha(0.14), -1.0, 0.0, 0.0, false),
        glass_shadow(white.with_alpha(0.10), 1.0, 0.0, 0.0, true),
        glass_shadow(black.with_alpha(0.22), -1.0, 0.0, 0.0, true),
        glass_shadow(black.with_alpha(0.70), 18.0, 48.0, -18.0, false),
        glass_shadow(white.with_alpha(0.08), 0.0, 0.0, 0.5, false),
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
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius))
            .child(
                div()
                    .relative()
                    .text_size(TypographyTokens::size(TextStep::X3s))
                    .line_height(px(16.0))
                    .font_features(FEATURES.clone())
                    .text_color(self.theme.colors.foreground.to_paint())
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
}
impl ComposerFrame {
    pub fn new(theme: ArtisanTheme, content: impl IntoElement) -> Self {
        Self {
            theme,
            content: content.into_any_element(),
            selector: None,
        }
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
            .shadow(glass_card_shadows())
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius))
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
    pub card_edge: Vec<BoxShadow>,
}
impl TabCardMaterial {
    pub fn new(theme: &ArtisanTheme) -> Self {
        let white = Oklch::new(1.0, 0.0, 0.0);
        let gradient = |a: Oklch, b: Oklch, alpha: f32| {
            linear_gradient(
                145.0,
                linear_color_stop(a.with_alpha(alpha).to_paint(), 0.0),
                linear_color_stop(b.with_alpha(alpha).to_paint(), 1.0),
            )
        };
        let highlight = |alpha, end| {
            linear_gradient(
                180.0,
                linear_color_stop(white.with_alpha(alpha).to_paint(), 0.0),
                linear_color_stop(white.with_alpha(0.0).to_paint(), end),
            )
        };
        Self {
            blur: glass_blur_radius(GlassStrength::Strong),
            shell: gradient(SurfaceStep::S800.oklch(), SurfaceStep::S900.oklch(), 0.62),
            strip: gradient(SurfaceStep::S700.oklch(), SurfaceStep::S800.oklch(), 0.62),
            highlight: highlight(0.035, 0.35),
            well: Oklch::new(0.164, 0.0, 0.0).to_paint(),
            ring: SurfaceStep::S50.oklch().with_alpha(0.04).to_paint(),
            top_light: highlight(0.04, 0.4),
            shadow: vec![BoxShadow {
                color: Oklch::new(0.0, 0.0, 0.0).with_alpha(0.6).to_paint(),
                offset: point(px(0.0), px(18.0)),
                blur_radius: px(48.0),
                spread_radius: px(-18.0),
                inset: false,
            }],
            card_edge: card_shadows(theme),
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
