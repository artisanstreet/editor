//! The stacked AR/TIS/AN wordmark as the application's loading mark.
//!
//! Native counterpart of the legacy connection overlay's loader
//! (`forge-connection-overlay.svelte`): the wordmark renders dimmed and
//! constant, and a copy of the same mark in the highlight colour shows
//! through a band that sweeps across it, so the letters never lose contrast.
//! The band is a row of narrow clips over exactly aligned copies, their
//! opacities following one smooth profile, because a vendored SVG paints one
//! flat colour. Reduced motion rests on the full-contrast mark with no
//! animation frames requested.

use std::time::Duration;

use gpui::{
    Animation, AnimationExt as _, App, Div, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, Styled as _, Window, div, px,
};

use artisan_assets::AssetId;

use crate::asset_seam::asset_glyph;
use crate::motion::{MotionPolicy, SHIMMER_CYCLE};
use crate::theme::ArtisanTheme;

/// Width over height of the wordmark artwork (its `viewBox`).
const WORDMARK_ASPECT: f32 = 136.675 / 225.15;

/// Mark height per unit of the legacy component's `size` (its row font
/// size): the three rows span 2.2515 em of ink.
const WORDMARK_HEIGHT_PER_SIZE: f32 = 2.2515;

/// The legacy component's default `size`.
const DEFAULT_SIZE: f32 = 48.0;

/// Band width as a fraction of the mark's width.
const BAND_FRACTION: f32 = 0.6;

/// Clips the band is built from; odd, so one sits at the crest.
const BAND_SLICES: usize = 9;

/// How long the mark takes to fade in once its delay has passed.
const ENTRANCE_FADE: Duration = Duration::from_millis(200);

/// The semantic label of the loading mark.
pub const WORDMARK_LOADER_LABEL: &str = "Loading";

/// The shimmering stacked wordmark.
#[derive(IntoElement)]
pub struct WordmarkLoader {
    id: SharedString,
    theme: ArtisanTheme,
    size: f32,
    motion_policy: MotionPolicy,
    debug_selector: Option<SharedString>,
    appear_after: Option<Duration>,
}

impl WordmarkLoader {
    /// Constructs the mark at the legacy default size with full motion.
    ///
    /// `id` keys the band's animation state and must be unique within the
    /// rendered tree.
    #[must_use]
    pub fn new(id: impl Into<SharedString>, theme: ArtisanTheme) -> Self {
        Self {
            id: id.into(),
            theme,
            size: DEFAULT_SIZE,
            motion_policy: MotionPolicy::Full,
            debug_selector: None,
            appear_after: None,
        }
    }

    /// Keeps the mark invisible for `delay` after it mounts, then fades it
    /// in, so a wait that ends quickly shows nothing rather than a flash of
    /// the mark. Reduced motion still waits, then shows the mark at once.
    #[must_use]
    pub const fn appear_after(mut self, delay: Duration) -> Self {
        self.appear_after = Some(delay);
        self
    }

    /// Sets the legacy `size` (the row font size in pixels); the whole mark
    /// scales from it.
    #[must_use]
    pub const fn size(mut self, size: f32) -> Self {
        self.size = size;
        self
    }

    /// Selects the shared full/reduced motion policy.
    #[must_use]
    pub const fn motion_policy(mut self, policy: MotionPolicy) -> Self {
        self.motion_policy = policy;
        self
    }

    /// Names the mark for tests and tooling.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// The painted box of a mark of the given `size`.
    #[must_use]
    pub fn dimensions(size: f32) -> (Pixels, Pixels) {
        let height = size * WORDMARK_HEIGHT_PER_SIZE;
        (px(height * WORDMARK_ASPECT), px(height))
    }
}

/// The mark's opacity `elapsed` after it mounted when it appears after
/// `delay`: nothing until the delay has passed, then a fade over
/// [`ENTRANCE_FADE`] (or a step, under reduced motion).
#[must_use]
pub fn entrance_opacity(elapsed: Duration, delay: Duration, reduced_motion: bool) -> f32 {
    let Some(since_delay) = elapsed.checked_sub(delay) else {
        return 0.0;
    };
    if reduced_motion {
        return 1.0;
    }
    (since_delay.as_secs_f32() / ENTRANCE_FADE.as_secs_f32()).clamp(0.0, 1.0)
}

/// The band's opacity at `position` in `[0, 1]` across its width: zero at
/// both edges, one at the crest.
#[must_use]
pub fn band_opacity(position: f32) -> f32 {
    (position.clamp(0.0, 1.0) * std::f32::consts::TAU)
        .cos()
        .mul_add(-0.5, 0.5)
}

/// The band's left edge at `progress` in `[0, 1]`: from fully left of the
/// mark to fully right of it.
#[must_use]
pub fn band_left(progress: f32, mark_width: f32, band_width: f32) -> f32 {
    (mark_width + band_width).mul_add(progress.clamp(0.0, 1.0), -band_width)
}

fn mark(width: Pixels, height: Pixels) -> Div {
    div().absolute().top_0().left_0().w(width).h(height).child(
        asset_glyph(AssetId::ARTISAN_WORDMARK_STACKED)
            .w(width)
            .h(height),
    )
}

impl RenderOnce for WordmarkLoader {
    #[expect(
        clippy::cast_precision_loss,
        reason = "slice indices are single digits, far inside f32's exact integer range"
    )]
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let (width, height) = Self::dimensions(self.size);
        let foreground = self.theme.colors.foreground.to_paint();
        let mut root = div().relative().flex_shrink_0().w(width).h(height);
        if let Some(selector) = self.debug_selector {
            root = root.debug_selector(move || selector.to_string());
        }
        let reduced = self.motion_policy == MotionPolicy::Reduced;
        let entrance = self.appear_after.map(|delay| {
            let id = ElementId::Name(SharedString::from(format!("{}-entrance", self.id)));
            let span = delay + ENTRANCE_FADE;
            (id, span, delay)
        });
        // Delayed entrance wraps whatever the policy paints; the wrapper's
        // animation is keyed like the band's, so it restarts per mount.
        let entered = |root: Div| match entrance {
            None => root.into_any_element(),
            Some((id, span, delay)) => root
                .opacity(0.0)
                .with_animation(id, Animation::new(span), move |root, progress| {
                    root.opacity(entrance_opacity(span.mul_f32(progress), delay, reduced))
                })
                .into_any_element(),
        };
        if reduced {
            return entered(root.text_color(foreground).child(mark(width, height)));
        }
        root = root
            .text_color(self.theme.colors.muted_foreground.to_paint())
            .child(mark(width, height));
        let mark_width = f32::from(width);
        let band_width = mark_width * BAND_FRACTION;
        let slice_width = band_width / BAND_SLICES as f32;
        for slice in 0..BAND_SLICES {
            let offset = slice_width * slice as f32;
            let opacity = band_opacity((slice as f32 + 0.5) / BAND_SLICES as f32);
            let left = move |progress: f32| band_left(progress, mark_width, band_width) + offset;
            // The clip travels right while the mark inside it travels left
            // by the same amount, so the highlighted letters stay exactly
            // over the dim ones.
            let inner = mark(width, height).text_color(foreground).with_animation(
                ElementId::Name(SharedString::from(format!("{}-mark-{slice}", self.id))),
                Animation::new(SHIMMER_CYCLE).repeat(),
                move |inner, progress| inner.left(px(-left(progress))),
            );
            root = root.child(
                div()
                    .absolute()
                    .top_0()
                    .w(px(slice_width))
                    .h(height)
                    .overflow_hidden()
                    .opacity(opacity)
                    .child(inner)
                    .with_animation(
                        ElementId::Name(SharedString::from(format!("{}-clip-{slice}", self.id))),
                        Animation::new(SHIMMER_CYCLE).repeat(),
                        move |clip, progress| clip.left(px(left(progress))),
                    ),
            );
        }
        entered(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_stays_hidden_through_its_delay_then_fades_in() {
        let delay = Duration::from_secs(1);
        assert!(entrance_opacity(Duration::ZERO, delay, false).abs() < f32::EPSILON);
        assert!(entrance_opacity(Duration::from_millis(999), delay, false).abs() < f32::EPSILON);
        let half = entrance_opacity(Duration::from_millis(1_100), delay, false);
        assert!((half - 0.5).abs() < 1e-3);
        assert!((entrance_opacity(Duration::from_millis(1_200), delay, false) - 1.0).abs() < 1e-6);
        assert!((entrance_opacity(Duration::from_secs(5), delay, false) - 1.0).abs() < 1e-6);
        // Reduced motion waits the same delay and then shows the mark whole.
        assert!(entrance_opacity(Duration::from_millis(999), delay, true).abs() < f32::EPSILON);
        assert!((entrance_opacity(Duration::from_millis(1_000), delay, true) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn the_mark_scales_from_the_legacy_size() {
        let (width, height) = WordmarkLoader::dimensions(48.0);
        assert!((f32::from(height) - 108.072).abs() < 0.01);
        assert!((f32::from(width) - 65.604).abs() < 0.01);
    }

    #[test]
    fn the_band_enters_and_leaves_fully_outside_the_mark() {
        assert!((band_left(0.0, 100.0, 60.0) + 60.0).abs() < f32::EPSILON);
        assert!((band_left(1.0, 100.0, 60.0) - 100.0).abs() < f32::EPSILON);
        assert!((band_left(0.5, 100.0, 60.0) - 20.0).abs() < 1e-4);
    }

    #[test]
    fn the_band_is_soft_at_its_edges_and_full_at_its_crest() {
        assert!(band_opacity(0.0).abs() < 1e-6);
        assert!(band_opacity(1.0).abs() < 1e-5);
        assert!((band_opacity(0.5) - 1.0).abs() < 1e-6);
        assert!(band_opacity(0.25) > 0.4 && band_opacity(0.25) < 0.6);
    }
}
