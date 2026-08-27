//! Native GPUI loading/status arc for the reached editor surfaces.
//!
//! This is the native counterpart of the audited `FadeArc` wrapper used by
//! `thread-route-gate.svelte` and `sidebar-engine-usage.svelte`. It keeps the
//! visual idea of two fading circular halves while using GPUI's real canvas,
//! path tessellation, and path-paint APIs. The component owns no accessibility
//! tree because pinned GPUI 0.2.2 has none; its status label and active state
//! remain first-party state for a future accessibility layer.
//!
//! Ported from loading-ui's `FadeArc` (loading-ui.com).
//!
//! The original component uses two per-instance SVG gradient ids. GPUI paths
//! carry their own `Background`, so this implementation has no global gradient
//! namespace to collide in. The caller-provided element id is used only for
//! GPUI's animation state and must be unique within the rendered tree.

use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

use gpui::prelude::Refineable;
use gpui::{
    Animation, AnimationExt, App, Background, Bounds, ElementId, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Path, PathBuilder, Pixels, Point, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, canvas, div, linear_color_stop, linear_gradient, point, px,
};

use crate::motion::MotionPolicy;
use crate::theme::ArtisanTheme;

/// The default square edge, matching the reached route-gate `size-6` use.
pub const DEFAULT_SIZE: Pixels = px(24.0);

/// The default CSS variable fallback from the audited wrapper.
pub const DEFAULT_DURATION: Duration = Duration::from_secs(1);

/// The semantic label retained by a default loading arc.
pub const DEFAULT_STATUS_LABEL: &str = "Loading";

/// The normalized starting phase for a newly constructed arc.
pub const DEFAULT_PHASE: f32 = 0.0;

/// The ratio that preserves the audited 3 px stroke over a 24 px view box.
pub const STROKE_WIDTH_RATIO: f32 = 3.0 / 24.0;

/// The first logical arc starts at the top and proceeds clockwise.
pub const LEADING_START_DEGREES: f32 = -90.0;

/// The leading arc is the stronger, longer half of the comet silhouette.
pub const LEADING_SWEEP_DEGREES: f32 = 210.0;

/// The trailing arc closes the circle with the shorter fading half.
pub const TRAILING_SWEEP_DEGREES: f32 = 150.0;

/// The trailing stop's nonzero alpha, transcribed from the SVG wrapper.
pub const FADE_ALPHA: f32 = 0.55;

/// Normalizes a cyclic phase into the half-open interval `[0, 1)`.
///
/// Non-finite input has no meaningful position on a clock and resolves to the
/// deterministic start phase instead of poisoning path geometry.
#[must_use]
pub fn normalize_phase(phase: f32) -> f32 {
    if phase.is_finite() {
        phase.rem_euclid(1.0)
    } else {
        DEFAULT_PHASE
    }
}

/// Advances an initial phase by elapsed time on a repeating duration.
///
/// A zero duration is treated as a static clock. This keeps callers from
/// constructing the invalid zero-duration GPUI animation that would divide by
/// zero internally, while preserving the requested phase visibly.
#[must_use]
pub fn phase_at(elapsed: Duration, duration: Duration, initial_phase: f32) -> f32 {
    if duration.is_zero() {
        return normalize_phase(initial_phase);
    }

    let progress = elapsed.as_secs_f32() / duration.as_secs_f32();
    if progress.is_finite() {
        normalize_phase(normalize_phase(initial_phase) + progress)
    } else {
        normalize_phase(initial_phase)
    }
}

/// Advances a phase by an animation progress sample in `[0, 1]`.
#[must_use]
pub fn phase_at_progress(initial_phase: f32, progress: f32) -> f32 {
    if progress.is_finite() {
        normalize_phase(normalize_phase(initial_phase) + progress)
    } else {
        normalize_phase(initial_phase)
    }
}

/// Converts a normalized phase into the clockwise rotation used by the arc.
#[must_use]
pub fn rotation_degrees(phase: f32) -> f32 {
    normalize_phase(phase) * 360.0
}

/// The two independently painted logical portions of the `FadeArc`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FadeArcSegment {
    /// The longer portion, opaque at its leading stop and fading toward its end.
    Leading,
    /// The shorter portion, transparent at its leading stop and fading in toward its end.
    Trailing,
}

/// One linear gradient used by a stroked arc path.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeArcGradient {
    /// Color at the first stop.
    pub start: Hsla,
    /// Color at the second stop.
    pub end: Hsla,
}

impl FadeArcGradient {
    /// Builds the GPUI linear background used by this path.
    ///
    /// GPUI's path shader samples the gradient over the tessellated path's
    /// bounds. A vertical gradient matches the source SVG's vertical stops
    /// without introducing a global id or a compatibility SVG element.
    #[must_use]
    pub fn background(self) -> Background {
        linear_gradient(
            0.0,
            linear_color_stop(self.start, 0.0),
            linear_color_stop(self.end, 1.0),
        )
    }
}

/// Theme-resolved paint for both `FadeArc` paths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeArcStyle {
    /// The inherited-equivalent current-color paint selected from the theme.
    pub color: Hsla,
    /// Gradient for the longer leading path.
    pub leading: FadeArcGradient,
    /// Gradient for the shorter trailing path.
    pub trailing: FadeArcGradient,
}

impl FadeArcStyle {
    /// Resolves the muted foreground used by both audited call sites.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self::from_color(theme.colors.muted_foreground.to_paint())
    }

    /// Builds the two source-equivalent fades from an explicit paint color.
    #[must_use]
    pub fn from_color(color: Hsla) -> Self {
        Self {
            color,
            leading: FadeArcGradient {
                start: color,
                end: color.opacity(FADE_ALPHA),
            },
            trailing: FadeArcGradient {
                start: color.opacity(0.0),
                end: color.opacity(FADE_ALPHA),
            },
        }
    }

    /// Replaces the theme-resolved current-color equivalent and recomputes both fades.
    #[must_use]
    pub fn with_color(self, color: Hsla) -> Self {
        Self::from_color(color)
    }
}

/// Deterministic circular geometry for one square arc viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeArcGeometry {
    /// The usable square side in pixels.
    pub size: Pixels,
    /// The absolute center used by the canvas paint pass.
    pub center: Point<Pixels>,
    /// Center-line radius, leaving half the stroke inside the viewport.
    pub radius: Pixels,
    /// Stroke width scaled from the audited 24 px view box.
    pub stroke_width: Pixels,
}

impl FadeArcGeometry {
    /// Resolves centered geometry for a square size at local origin.
    #[must_use]
    pub fn for_size(size: Pixels) -> Self {
        let edge = finite_dimension(size);
        let stroke_width = edge * STROKE_WIDTH_RATIO;
        let radius = ((edge - stroke_width) / 2.0).max(0.0);

        Self {
            size: px(edge),
            center: point(px(edge / 2.0), px(edge / 2.0)),
            radius: px(radius),
            stroke_width: px(stroke_width),
        }
    }

    /// Resolves geometry from the smaller dimension of actual canvas bounds.
    #[must_use]
    pub fn for_bounds(bounds: Bounds<Pixels>) -> Self {
        let side = finite_dimension(bounds.size.width)
            .min(finite_dimension(bounds.size.height))
            .max(0.0);
        let mut geometry = Self::for_size(px(side));

        geometry.center = point(
            bounds.origin.x + px(side / 2.0),
            bounds.origin.y + px(side / 2.0),
        );
        geometry
    }

    /// Returns the rotated start and end angles for one logical segment.
    #[must_use]
    pub fn segment_angles(self, segment: FadeArcSegment, phase: f32) -> (f32, f32) {
        let rotation = rotation_degrees(phase);

        match segment {
            FadeArcSegment::Leading => (
                LEADING_START_DEGREES + rotation,
                LEADING_START_DEGREES + LEADING_SWEEP_DEGREES + rotation,
            ),
            FadeArcSegment::Trailing => (
                LEADING_START_DEGREES + LEADING_SWEEP_DEGREES + rotation,
                LEADING_START_DEGREES + LEADING_SWEEP_DEGREES + TRAILING_SWEEP_DEGREES + rotation,
            ),
        }
    }
}

/// Semantic state retained by a `FadeArc` for future accessibility integration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FadeArcState {
    active: bool,
    status_label: SharedString,
}

impl Default for FadeArcState {
    fn default() -> Self {
        Self {
            active: true,
            status_label: DEFAULT_STATUS_LABEL.into(),
        }
    }
}

impl FadeArcState {
    /// Whether the caller considers the represented work active.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// The semantic status label kept verbatim for a future accessibility layer.
    #[must_use]
    pub fn status_label(&self) -> &str {
        self.status_label.as_str()
    }
}

/// Copyable animation specification kept separate from GPUI's runtime object.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FadeArcAnimation {
    duration: Duration,
    initial_phase: f32,
}

impl FadeArcAnimation {
    /// Creates a valid repeating animation specification.
    #[must_use]
    pub fn new(duration: Duration, initial_phase: f32) -> Option<Self> {
        (!duration.is_zero()).then_some(Self {
            duration,
            initial_phase: normalize_phase(initial_phase),
        })
    }

    /// The full rotation duration.
    #[must_use]
    pub const fn duration(self) -> Duration {
        self.duration
    }

    /// The phase from which the animation begins.
    #[must_use]
    pub const fn initial_phase(self) -> f32 {
        self.initial_phase
    }

    /// Samples the deterministic rotation phase for a GPUI progress value.
    #[must_use]
    pub fn phase_at_progress(self, progress: f32) -> f32 {
        phase_at_progress(self.initial_phase, progress)
    }

    /// Creates the repeating, linear-clock GPUI animation.
    #[must_use]
    pub fn gpui_animation(self) -> Animation {
        Animation::new(self.duration).repeat()
    }
}

/// The motion decision for one configured `FadeArc`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FadeArcMotionPlan {
    /// Paint the arc at one fixed phase without requesting animation frames.
    Static { phase: f32 },
    /// Advance the arc with the repeating animation specification.
    Animate(FadeArcAnimation),
}

impl FadeArcMotionPlan {
    /// Resolves active/reduced/zero-duration state without constructing a runtime animation.
    #[must_use]
    pub fn for_state(
        active: bool,
        policy: MotionPolicy,
        duration: Duration,
        initial_phase: f32,
    ) -> Self {
        if active
            && matches!(policy, MotionPolicy::Full)
            && let Some(animation) = FadeArcAnimation::new(duration, initial_phase)
        {
            Self::Animate(animation)
        } else {
            Self::Static {
                phase: normalize_phase(initial_phase),
            }
        }
    }

    /// Whether this plan advances and requests GPUI animation frames.
    #[must_use]
    pub const fn is_animated(self) -> bool {
        matches!(self, Self::Animate(_))
    }

    /// Returns the static phase or the animation's initial phase.
    #[must_use]
    pub const fn phase(self) -> f32 {
        match self {
            Self::Static { phase } => phase,
            Self::Animate(animation) => animation.initial_phase,
        }
    }

    /// Returns the copyable animation specification when motion is active.
    #[must_use]
    pub const fn animation(self) -> Option<FadeArcAnimation> {
        match self {
            Self::Static { .. } => None,
            Self::Animate(animation) => Some(animation),
        }
    }
}

/// A native, noninteractive two-segment fading GPUI arc.
///
/// `id` is deliberately required because GPUI stores repeating animation
/// state by element id. Give each live instance a distinct id, just as any
/// other stateful GPUI element would. The id is never used for gradient lookup:
/// each path carries its own gradient value.
#[derive(IntoElement)]
pub struct FadeArc {
    id: ElementId,
    style: FadeArcStyle,
    size: Pixels,
    duration: Duration,
    motion_policy: MotionPolicy,
    initial_phase: f32,
    state: FadeArcState,
    style_refinement: StyleRefinement,
    debug_selector: Option<SharedString>,
}

impl FadeArc {
    /// Constructs a theme-resolved arc with full motion and default semantics.
    #[must_use]
    pub fn new(id: impl Into<ElementId>, theme: ArtisanTheme) -> Self {
        Self {
            id: id.into(),
            style: FadeArcStyle::resolve(theme),
            size: DEFAULT_SIZE,
            duration: DEFAULT_DURATION,
            motion_policy: MotionPolicy::Full,
            initial_phase: DEFAULT_PHASE,
            state: FadeArcState::default(),
            style_refinement: StyleRefinement::default(),
            debug_selector: None,
        }
    }

    /// Overrides the square canvas size.
    #[must_use]
    pub fn size(mut self, size: Pixels) -> Self {
        self.size = px(finite_dimension(size));
        self
    }

    /// Overrides the repeating rotation duration.
    ///
    /// A zero duration is retained as an explicit value but resolves to a
    /// static plan, avoiding GPUI's invalid zero-duration clock.
    #[must_use]
    pub const fn duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Sets whether this status treatment represents active work.
    ///
    /// Inactive arcs remain painted at their current configured phase but do
    /// not request animation frames.
    #[must_use]
    pub const fn active(mut self, active: bool) -> Self {
        self.state.active = active;
        self
    }

    /// Selects the shared full/reduced motion policy.
    #[must_use]
    pub const fn motion_policy(mut self, policy: MotionPolicy) -> Self {
        self.motion_policy = policy;
        self
    }

    /// Sets the deterministic starting phase, normalized into `[0, 1)`.
    #[must_use]
    pub fn phase(mut self, phase: f32) -> Self {
        self.initial_phase = normalize_phase(phase);
        self
    }

    /// Replaces the semantic status label without changing visible geometry.
    #[must_use]
    pub fn status_label(mut self, label: impl Into<SharedString>) -> Self {
        self.state.status_label = label.into();
        self
    }

    /// Replaces the resolved visual gradients with an explicit paint color.
    #[must_use]
    pub fn color(mut self, color: Hsla) -> Self {
        self.style = self.style.with_color(color);
        self
    }

    /// Replaces the complete resolved visual style.
    #[must_use]
    pub const fn with_visual_style(mut self, style: FadeArcStyle) -> Self {
        self.style = style;
        self
    }

    /// Adds a stable debug selector to the actual painted canvas element.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the caller-provided GPUI identity.
    #[must_use]
    pub const fn element_id(&self) -> &ElementId {
        &self.id
    }

    /// Returns the child identity used by the repeating animation.
    #[must_use]
    pub fn animation_id(&self) -> ElementId {
        animation_element_id(&self.id)
    }

    /// Returns the resolved geometry-independent visual style.
    #[must_use]
    pub const fn visual_style(&self) -> FadeArcStyle {
        self.style
    }

    /// Returns the semantic state retained for this instance.
    #[must_use]
    pub const fn state(&self) -> &FadeArcState {
        &self.state
    }

    /// Returns the configured size before caller style refinements.
    #[must_use]
    pub const fn size_value(&self) -> Pixels {
        self.size
    }

    /// Returns the configured duration before motion-plan resolution.
    #[must_use]
    pub const fn duration_value(&self) -> Duration {
        self.duration
    }

    /// Returns whether this instance represents active work.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.state.active
    }

    /// Returns the retained semantic label.
    #[must_use]
    pub fn status_label_value(&self) -> &str {
        self.state.status_label()
    }

    /// Returns the configured motion policy.
    #[must_use]
    pub const fn motion_policy_value(&self) -> MotionPolicy {
        self.motion_policy
    }

    /// Returns the normalized configured starting phase.
    #[must_use]
    pub const fn initial_phase(&self) -> f32 {
        self.initial_phase
    }

    /// Resolves the current state into a static or animated plan.
    #[must_use]
    pub fn motion_plan(&self) -> FadeArcMotionPlan {
        FadeArcMotionPlan::for_state(
            self.state.active,
            self.motion_policy,
            self.duration,
            self.initial_phase,
        )
    }
}

impl Styled for FadeArc {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style_refinement
    }
}

impl RenderOnce for FadeArc {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let motion = self.motion_plan();
        let initial_phase = self.initial_phase;
        let style = self.style;
        let caller_style = self.style_refinement;
        let animation_id = animation_element_id(&self.id);
        let phase = Rc::new(Cell::new(initial_phase));
        let phase_for_prepaint = Rc::clone(&phase);

        let canvas = canvas(
            move |bounds, _, _| build_paint(bounds, style, phase_for_prepaint.get()),
            move |_, paint, window, _| paint.paint(window),
        )
        .size_full();

        let mut element = div().w(self.size).h(self.size).child(canvas);

        element.style().refine(&caller_style);

        if let Some(selector) = self.debug_selector {
            let selector = selector.to_string();
            element = element.debug_selector(move || selector.clone());
        }

        match motion {
            FadeArcMotionPlan::Static {
                phase: static_phase,
            } => {
                phase.set(static_phase);
                element.into_any_element()
            }
            FadeArcMotionPlan::Animate(animation) => {
                let phase_for_animation = Rc::clone(&phase);
                element
                    .with_animation(
                        animation_id,
                        animation.gpui_animation(),
                        move |element, progress| {
                            phase_for_animation.set(animation.phase_at_progress(progress));
                            element
                        },
                    )
                    .into_any_element()
            }
        }
    }
}

/// Constructs a native `FadeArc` from a stable GPUI identity and theme.
#[must_use]
pub fn fade_arc(id: impl Into<ElementId>, theme: ArtisanTheme) -> FadeArc {
    FadeArc::new(id, theme)
}

struct FadeArcPaint {
    leading: Option<Path<Pixels>>,
    trailing: Option<Path<Pixels>>,
    leading_background: Background,
    trailing_background: Background,
}

impl FadeArcPaint {
    fn paint(self, window: &mut Window) {
        if let Some(path) = self.leading {
            window.paint_path(path, self.leading_background);
        }
        if let Some(path) = self.trailing {
            window.paint_path(path, self.trailing_background);
        }
    }
}

fn build_paint(bounds: Bounds<Pixels>, style: FadeArcStyle, phase: f32) -> FadeArcPaint {
    let geometry = FadeArcGeometry::for_bounds(bounds);

    FadeArcPaint {
        leading: build_segment_path(&geometry, FadeArcSegment::Leading, phase),
        trailing: build_segment_path(&geometry, FadeArcSegment::Trailing, phase),
        leading_background: style.leading.background(),
        trailing_background: style.trailing.background(),
    }
}

fn build_segment_path(
    geometry: &FadeArcGeometry,
    segment: FadeArcSegment,
    phase: f32,
) -> Option<Path<Pixels>> {
    if f32::from(geometry.size) <= 0.0 || f32::from(geometry.radius) <= 0.0 {
        return None;
    }

    let (start_degrees, end_degrees) = geometry.segment_angles(segment, phase);
    let mut builder = PathBuilder::stroke(geometry.stroke_width);

    builder.move_to(point_on_circle(
        geometry.center,
        geometry.radius,
        start_degrees,
    ));
    builder.arc_to(
        point(geometry.radius, geometry.radius),
        px(0.0),
        matches!(segment, FadeArcSegment::Leading),
        true,
        point_on_circle(geometry.center, geometry.radius, end_degrees),
    );

    builder.build().ok()
}

fn point_on_circle(center: Point<Pixels>, radius: Pixels, degrees: f32) -> Point<Pixels> {
    let radians = degrees.to_radians();
    let radius = f32::from(radius);

    point(
        center.x + px(radius * radians.cos()),
        center.y + px(radius * radians.sin()),
    )
}

fn finite_dimension(value: Pixels) -> f32 {
    let value = f32::from(value);

    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn animation_element_id(id: &ElementId) -> ElementId {
    ElementId::NamedChild(Box::new(id.clone()), "spin".into())
}
