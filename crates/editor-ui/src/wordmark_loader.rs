//! The application loading mark uses one canvas and one cached SVG mask.
//! Reduced motion shows the full-contrast mark immediately.

use std::time::Duration;

use gpui::{
    App, Bounds, ContentMask, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, canvas, div,
    point, px, size,
};

use editor_assets::AssetId;

use crate::motion::{MotionPolicy, SHIMMER_CYCLE};
use crate::theme::ArtisanTheme;

/// Width over height of the wordmark artwork (its `viewBox`).
const WORDMARK_ASPECT: f32 = 136.675 / 225.15;

/// Mark height per unit of row font size; three rows span 2.2515 em.
const WORDMARK_HEIGHT_PER_SIZE: f32 = 2.2515;

/// Default row font size in pixels.
const DEFAULT_SIZE: f32 = 48.0;

/// Band width as a fraction of the mark's width.
const BAND_FRACTION: f32 = 0.6;

/// Clips the band is built from; odd, so one sits at the crest.
const BAND_SLICES: usize = 9;

/// How long the mark takes to fade in once its delay has passed.
const ENTRANCE_FADE: Duration = Duration::from_millis(200);

/// The semantic label of the loading mark.
const WORDMARK_LOADER_LABEL: &str = "Loading";

/// The shimmering stacked wordmark.
#[derive(IntoElement)]
pub struct WordmarkLoader {
    id: SharedString,
    theme: ArtisanTheme,
    appear_after: Option<Duration>,
}

impl WordmarkLoader {
    /// Constructs the mark. Motion follows the application preference.
    ///
    /// `id` keys the band's animation state and must be unique within the
    /// rendered tree.
    #[must_use]
    pub fn new(id: impl Into<SharedString>, theme: ArtisanTheme) -> Self {
        Self {
            id: id.into(),
            theme,
            appear_after: None,
        }
    }

    /// Keeps the mark invisible for `delay` after it mounts, then fades it
    /// in, so a wait that ends quickly shows nothing rather than a flash of
    /// the mark. Reduced motion shows the mark immediately.
    #[must_use]
    pub const fn appear_after(mut self, delay: Duration) -> Self {
        self.appear_after = Some(delay);
        self
    }
}

/// Opacity after the appearance delay has elapsed.
fn entrance_opacity(elapsed: Duration) -> f32 {
    (elapsed.as_secs_f32() / ENTRANCE_FADE.as_secs_f32()).clamp(0.0, 1.0)
}

/// The band's opacity at `position` in `[0, 1]` across its width: zero at
/// both edges, one at the crest.
#[must_use]
fn band_opacity(position: f32) -> f32 {
    (position.clamp(0.0, 1.0) * std::f32::consts::TAU)
        .cos()
        .mul_add(-0.5, 0.5)
}

/// The band's left edge at `progress` in `[0, 1]`: from fully left of the
/// mark to fully right of it.
#[must_use]
fn band_left(progress: f32, mark_width: f32, band_width: f32) -> f32 {
    (mark_width + band_width).mul_add(progress.clamp(0.0, 1.0), -band_width)
}

impl RenderOnce for WordmarkLoader {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let height = px(DEFAULT_SIZE * WORDMARK_HEIGHT_PER_SIZE);
        let width = height * WORDMARK_ASPECT;
        let mut root = div()
            .id(self.id.clone())
            .role(gpui::Role::ProgressIndicator)
            .aria_label(WORDMARK_LOADER_LABEL)
            .relative()
            .flex_shrink_0()
            .w(width)
            .h(height);
        let reduced = MotionPolicy::current(cx) == MotionPolicy::Reduced;
        let theme = self.theme;
        let path: SharedString = AssetId::ARTISAN_WORDMARK_STACKED.as_str().into();
        if reduced {
            return root.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let color = theme.colors.text.to_paint();
                        // A failed paint leaves this element blank for one frame.
                        window
                            .paint_svg(
                                bounds,
                                path,
                                None,
                                gpui::TransformationMatrix::default(),
                                color,
                                cx,
                            )
                            .ok();
                    },
                )
                .size_full(),
            );
        }
        let Some(started) = crate::motion::started_after(
            SharedString::from(format!("{}-clock", self.id)),
            self.appear_after.unwrap_or_default(),
            window,
            cx,
        ) else {
            return root;
        };
        let fades = self.appear_after.is_some();
        // The entrance and the band are paint-only: they repaint without
        // rendering again.
        root = root.child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| {
                    window.paint_animated(
                        move |window, cx| {
                            let elapsed = started.elapsed();
                            let opacity = if fades {
                                entrance_opacity(elapsed)
                            } else {
                                1.0
                            };
                            let progress =
                                (elapsed.as_secs_f32() / SHIMMER_CYCLE.as_secs_f32()).fract();
                            paint_shimmer(bounds, &path, &theme, opacity, progress, window, cx);
                            !cx.reduce_motion()
                        },
                        cx,
                    );
                },
            )
            .size_full(),
        );
        root
    }
}

/// Paints the muted mark at `opacity` with the band at `progress` over it.
fn paint_shimmer(
    bounds: Bounds<Pixels>,
    path: &SharedString,
    theme: &ArtisanTheme,
    opacity: f32,
    progress: f32,
    window: &mut Window,
    cx: &App,
) {
    let foreground = theme.colors.text;
    let color = theme.colors.text_muted.fade(opacity).to_paint();
    // A failed paint leaves this element blank for one frame.
    window
        .paint_svg(
            bounds,
            path.clone(),
            None,
            gpui::TransformationMatrix::default(),
            color,
            cx,
        )
        .ok();
    let mark_width = f32::from(bounds.size.width);
    let band_width = mark_width * BAND_FRACTION;
    let slice_width = band_width / crate::f32_of_count(BAND_SLICES);
    let leading = band_left(progress, mark_width, band_width);
    for slice in 0..BAND_SLICES {
        let left = slice_width.mul_add(crate::f32_of_count(slice), leading);
        let clip = Bounds::new(
            point(bounds.left() + px(left), bounds.top()),
            size(px(slice_width), bounds.size.height),
        );
        let color = foreground
            .fade(
                opacity
                    * band_opacity(
                        (crate::f32_of_count(slice) + 0.5) / crate::f32_of_count(BAND_SLICES),
                    ),
            )
            .to_paint();
        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
            // A failed paint leaves this element blank for one frame.
            window
                .paint_svg(
                    bounds,
                    path.clone(),
                    None,
                    gpui::TransformationMatrix::default(),
                    color,
                    cx,
                )
                .ok();
        });
    }
}
