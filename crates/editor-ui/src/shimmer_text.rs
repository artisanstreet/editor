//! Status text with a travelling contrast band.
//! Tone, truncation, and timing belong to the component. Text stays outside
//! prose selection. Reduced motion and inactive states schedule no frames.

use std::{ops::Range, rc::Rc, time::Duration};

use gpui::{
    AnyElement, App, ElementId, HighlightStyle, Hsla, IntoElement, ParentElement, SharedString,
    Styled, StyledText, Window, div, hsla_to_rgba, rgb_to_hsla,
};

use crate::motion::{AnimatedPaint, MotionPolicy, RepeatingPhase, SHIMMER_CYCLE, SHIMMER_DELAY};
use crate::text_runs::{TextRunOverride, compile_text_runs};
use crate::theme::ArtisanTheme;

const MAX_SPREAD: f32 = 100.0;

#[derive(Clone, Copy, Default)]
pub enum ShimmerTone {
    #[default]
    Foreground,
    Muted,
}
#[derive(Clone, Copy, Default)]
pub enum ShimmerTiming {
    /// A delayed pass for an operation whose text may settle immediately.
    #[default]
    Status,
    /// An immediate pass for visible progress or a live summary.
    Progress,
}
impl ShimmerTiming {
    fn durations(self) -> (Duration, Duration) {
        match self {
            Self::Status => (SHIMMER_CYCLE, SHIMMER_DELAY),
            Self::Progress => (Duration::from_secs(2), Duration::ZERO),
        }
    }
}

/// Status text owns its tone, truncation and motion; it is outside prose selection.
#[derive(IntoElement)]
pub struct ShimmerText {
    id: ElementId,
    content: SharedString,
    theme: ArtisanTheme,
    timing: ShimmerTiming,
    tone: ShimmerTone,
    truncate: bool,
    active: bool,
    run_highlights: Vec<(Range<usize>, HighlightStyle)>,
    run_overrides: Vec<TextRunOverride>,
}
impl ShimmerText {
    pub fn new(
        id: impl Into<ElementId>,
        content: impl Into<SharedString>,
        theme: ArtisanTheme,
    ) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            theme,
            timing: ShimmerTiming::Status,
            tone: ShimmerTone::Foreground,
            truncate: false,
            active: true,
            run_highlights: Vec::new(),
            run_overrides: Vec::new(),
        }
    }
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
    pub fn timing(mut self, timing: ShimmerTiming) -> Self {
        self.timing = timing;
        self
    }
    pub fn tone(mut self, tone: ShimmerTone) -> Self {
        self.tone = tone;
        self
    }
    pub fn truncate(mut self) -> Self {
        self.truncate = true;
        self
    }
    pub fn text_runs(
        mut self,
        highlights: Vec<(Range<usize>, HighlightStyle)>,
        overrides: Vec<TextRunOverride>,
    ) -> Self {
        self.run_highlights = highlights;
        self.run_overrides = overrides;
        self
    }
}
impl gpui::RenderOnce for ShimmerText {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = match self.tone {
            ShimmerTone::Foreground => self.theme.colors.foreground.to_paint(),
            ShimmerTone::Muted => self.theme.colors.muted_foreground.to_paint(),
        };
        let mut inherited = window.text_style();
        inherited.color = color;
        let runs = compile_text_runs(
            &self.content,
            &inherited,
            &self.run_highlights,
            &self.run_overrides,
        );
        let text = StyledText::new(self.content)
            .with_runs(runs)
            .into_any_element();
        let mut root = div().min_w_0().text_color(color);
        if self.truncate {
            root = root.truncate();
        }
        if !self.active || MotionPolicy::current(cx) == MotionPolicy::Reduced {
            return root.child(text).into_any_element();
        }
        let (duration, delay) = self.timing.durations();
        let phase = crate::motion::repeating_phase(self.id, duration, delay, window, cx);
        root.child(gradient_text(text, phase, 50.0))
            .into_any_element()
    }
}

/// Moves a band across the text. Its flat center spans 40% through 60%;
/// both edges fade continuously to the base text color.
#[must_use]
fn gradient_strength(position: f32, phase: f32, spread: f32) -> f32 {
    let width = normalize_spread(spread) / MAX_SPREAD;
    if width <= 0.0 || !position.is_finite() {
        return 0.0;
    }
    let start = (1.0 - width) * (2.5 - 3.5 * normalize_phase(phase));
    let offset = (position - start) / width;
    (offset / 0.4).min((1.0 - offset) / 0.4).clamp(0.0, 1.0)
}

/// Mixes contrast in sRGB while preserving the text alpha.
#[must_use]
fn shimmer_color(base: Hsla, strength: f32) -> Hsla {
    if strength <= 0.0 {
        return base;
    }
    let (target, mix) = (0.0, 0.55);
    let amount = strength.clamp(0.0, 1.0) * mix;
    let mut color = hsla_to_rgba(base);
    color.red += (target - color.red) * amount;
    color.green += (target - color.green) * amount;
    color.blue += (target - color.blue) * amount;
    rgb_to_hsla(color)
}

/// Paints the text once, then recolours that paint under the band on every
/// frame of a pass without rendering or laying it out again.
fn gradient_text(text: AnyElement, phase: RepeatingPhase, spread: f32) -> AnimatedPaint {
    AnimatedPaint::new(text, move |bounds, recording, window, cx| {
        let sampled = phase.sample(cx);
        let width = f32::from(bounds.size.width);
        if width <= 0.0 || spread <= 0.0 {
            window.paint_recording(recording);
            return false;
        }
        let phase = sampled.unwrap_or(0.0);
        window.with_text_color_map(
            Some(Rc::new(move |x, color| {
                let position = f32::from(x - bounds.left()) / width;
                shimmer_color(color, gradient_strength(position, phase, spread))
            })),
            |window| window.paint_recording(recording),
        );
        sampled.is_some() && !cx.reduce_motion()
    })
}

fn normalize_phase(phase: f32) -> f32 {
    if phase.is_finite() {
        phase.rem_euclid(1.0)
    } else {
        0.0
    }
}
fn normalize_spread(spread: f32) -> f32 {
    if spread.is_finite() {
        spread.clamp(0.0, MAX_SPREAD)
    } else {
        0.0
    }
}
