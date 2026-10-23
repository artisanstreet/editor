//! Shared motion recipes. Components read the application preference at render time.

use std::time::Duration;

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, PaintRecording, Pixels, Window,
};

/// The explicit motion preference supplied by a product boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionPolicy {
    /// Use the selected recipe's full animation.
    Full,
    /// Reveal the final state immediately.
    Reduced,
}

impl MotionPolicy {
    /// Resolve the app preference at the component boundary.
    pub fn current(cx: &gpui::App) -> Self {
        if cx.reduce_motion() {
            Self::Reduced
        } else {
            Self::Full
        }
    }

    /// Resolves a semantic recipe under this explicit policy.
    #[must_use]
    pub const fn resolve(self, recipe: MotionRecipe) -> MotionPlan {
        match self {
            Self::Full => MotionPlan::Animate(recipe.animation()),
            Self::Reduced => MotionPlan::Immediate,
        }
    }
}

/// A policy decision that keeps immediate and animated presentation distinct.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotionPlan {
    /// Present the recipe's final state without constructing an animation.
    Immediate,
    /// Run the supplied positive-duration animation specification.
    Animate(MotionAnimation),
}

/// Shared duration tokens retained from the reachable motion system.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionDuration {
    /// 150 milliseconds.
    Quick,
    /// 250 milliseconds.
    Fast,
    /// 350 milliseconds.
    Medium,
}

impl MotionDuration {
    /// Returns the exact duration represented by this token.
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        match self {
            Self::Quick => Duration::from_millis(150),
            Self::Fast => Duration::from_millis(250),
            Self::Medium => Duration::from_millis(350),
        }
    }
}

/// The shimmer band's active travel duration.
pub const SHIMMER_CYCLE: Duration = Duration::from_secs(3);
/// The shimmer band's initial delay before its first cycle.
pub const SHIMMER_DELAY: Duration = Duration::from_millis(1_500);
/// One full fade-arc rotation.
pub const FADE_ARC_ROTATION: Duration = Duration::from_secs(1);
/// The selected visual easing curves, sampled independently from GPUI's clock.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionCurve {
    /// `cubic-bezier(0.22, 1, 0.36, 1)`.
    SmoothOut,
    /// CSS ease-out.
    EaseOut,
    /// `cubic-bezier(0.42, 0, 0.58, 1)`.
    EaseInOut,
}

impl MotionCurve {
    /// Samples this CSS cubic-bezier at an input progress value.
    ///
    /// CSS timing functions map input progress through the curve's x axis, so
    /// this solves `x(t) = progress` before evaluating `y(t)`. Values outside
    /// the clock interval clamp to the exact endpoints.
    #[must_use]
    pub fn sample(self, progress: f64) -> f64 {
        if progress <= 0.0 {
            return 0.0;
        }
        if progress >= 1.0 {
            return 1.0;
        }

        let (x1, y1, x2, y2) = self.control_points();
        let target_x = progress;
        let mut lower = 0.0_f64;
        let mut upper = 1.0_f64;

        // Every selected x control point lies in [0, 1], making x(t)
        // monotonic. A fixed bisection count is deterministic and avoids the
        // unstable derivative edge cases of a Newton-only solver. Twenty steps
        // resolve subpixel progress without calculating unused f64 precision.
        for _ in 0..20 {
            let t = lower.midpoint(upper);
            if cubic_axis(t, x1, x2) < target_x {
                lower = t;
            } else {
                upper = t;
            }
        }

        cubic_axis(lower.midpoint(upper), y1, y2)
    }

    /// [`Self::sample`] narrowed for paint values, which are `f32`.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the curve's 0..=1 output fits an f32"
    )]
    pub fn sample_f32(self, progress: f64) -> f32 {
        self.sample(progress) as f32
    }

    const fn control_points(self) -> (f64, f64, f64, f64) {
        match self {
            Self::SmoothOut => (0.22, 1.0, 0.36, 1.0),
            Self::EaseOut => (0.0, 0.0, 0.58, 1.0),
            Self::EaseInOut => (0.42, 0.0, 0.58, 1.0),
        }
    }
}

/// `cubic-bezier(0.22, 1, 0.36, 1)` as a GPUI easing.
#[must_use]
pub fn smooth_out(progress: f32) -> f32 {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the curve samples in f64 and feeds f32 paint values"
    )]
    let value = MotionCurve::SmoothOut.sample(f64::from(progress)) as f32;
    value
}

fn cubic_axis(t: f64, first: f64, second: f64) -> f64 {
    let inverse = 1.0 - t;
    (t * t).mul_add(
        t,
        (3.0 * inverse * t * t).mul_add(second, 3.0 * inverse * inverse * t * first),
    )
}

/// The shared copy-confirmation transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionAnimation {
    duration: Duration,
    curve: MotionCurve,
}
impl MotionAnimation {
    pub const fn duration(self) -> Duration {
        self.duration
    }
    pub const fn curve(self) -> MotionCurve {
        self.curve
    }
}
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionRecipe {
    IconSwap,
    AccordionExpand,
    TextSwap,
}
impl MotionRecipe {
    const fn animation(self) -> MotionAnimation {
        match self {
            Self::IconSwap => MotionAnimation {
                duration: MotionDuration::Fast.as_duration(),
                curve: MotionCurve::EaseInOut,
            },
            Self::AccordionExpand => MotionAnimation {
                duration: MotionDuration::Fast.as_duration(),
                curve: MotionCurve::SmoothOut,
            },
            Self::TextSwap => MotionAnimation {
                duration: MotionDuration::Quick.as_duration(),
                curve: MotionCurve::EaseInOut,
            },
        }
    }
}

/// A mounted clock sleeps during a stationary hold instead of scheduling frames.
struct PaintClock {
    since: std::time::Instant,
    waiting: bool,
    wake: Option<gpui::Task<()>>,
}
fn clock(
    key: impl Into<gpui::ElementId>,
    delay: Duration,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> gpui::Entity<PaintClock> {
    window.use_keyed_state(key, cx, |_, _| PaintClock {
        since: std::time::Instant::now(),
        waiting: !delay.is_zero(),
        wake: None,
    })
}
fn wake(state: &mut PaintClock, delay: Duration, cx: &gpui::Context<PaintClock>) {
    if state.wake.is_some() {
        return;
    }
    state.wake = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(delay).await;
        // A released view has nothing left to update.
        this.update(cx, |state, cx| {
            state.since = std::time::Instant::now();
            state.waiting = false;
            state.wake = None;
            cx.notify();
        })
        .ok();
    }));
}
/// When the mounted clock started, `None` while its `delay` holds. The
/// clock's view renders again once the delay has passed.
pub(crate) fn started_after(
    key: impl Into<gpui::ElementId>,
    delay: Duration,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> Option<std::time::Instant> {
    clock(key, delay, window, cx).update(cx, |state, cx| {
        if state.waiting {
            wake(state, delay, cx);
            None
        } else {
            Some(state.since)
        }
    })
}
/// A cycle that holds for its delay before every pass. Mounted at render,
/// sampled at paint time, so a retained paint can follow it without its
/// view rendering again.
pub(crate) struct RepeatingPhase {
    clock: gpui::Entity<PaintClock>,
    duration: Duration,
    delay: Duration,
}
impl RepeatingPhase {
    /// The unit phase of the current pass, `None` while it holds. Its view
    /// renders again when a hold ends.
    pub(crate) fn sample(&self, cx: &mut gpui::App) -> Option<f32> {
        let (duration, delay) = (self.duration, self.delay);
        self.clock.update(cx, |state, cx| {
            if !state.waiting && state.since.elapsed() >= duration {
                state.since = std::time::Instant::now();
                state.waiting = !delay.is_zero();
            }
            if state.waiting {
                wake(state, delay, cx);
                None
            } else {
                Some((state.since.elapsed().as_secs_f32() / duration.as_secs_f32()).min(1.))
            }
        })
    }
}
pub(crate) fn repeating_phase(
    key: impl Into<gpui::ElementId>,
    duration: Duration,
    delay: Duration,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> RepeatingPhase {
    RepeatingPhase {
        clock: clock(key, delay, window, cx),
        duration,
        delay,
    }
}

/// What [`AnimatedPaint`] repaints its child's recording with.
type Repaint = Box<dyn FnMut(Bounds<Pixels>, &PaintRecording, &mut Window, &mut App) -> bool>;

/// Paint-only motion over a child (`Window::paint_animated`): the child lays
/// out, prepaints and paints once into a recording; `repaint` paints that
/// recording within the child's bounds on every frame it asks for, without
/// its view rendering or laying out again. `repaint` derives its animated
/// value from the time, returns whether to paint again, and returns `false`
/// under reduced motion.
pub(crate) struct AnimatedPaint {
    child: AnyElement,
    repaint: Option<Repaint>,
}
impl AnimatedPaint {
    pub(crate) fn new(
        child: impl IntoElement,
        repaint: impl FnMut(Bounds<Pixels>, &PaintRecording, &mut Window, &mut App) -> bool + 'static,
    ) -> Self {
        Self {
            child: child.into_any_element(),
            repaint: Some(Box::new(repaint)),
        }
    }
}
impl IntoElement for AnimatedPaint {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}
impl Element for AnimatedPaint {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        (): &mut (),
        (): &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        #[expect(clippy::expect_used, reason = "GPUI paints an element once per frame")]
        let mut repaint = self.repaint.take().expect("an element paints once");
        let recording = window.record_paint(|window| self.child.paint(window, cx));
        window.paint_animated(
            move |window, cx| repaint(bounds, &recording, window, cx),
            cx,
        );
    }
}
