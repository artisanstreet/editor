//! A controlled native hover-card primitive for the reached `LinkPreview` path.
//!
//! The source contract is the context-usage gauge's LinkPreview.Root:
//! opening is immediate, closing waits 120 ms, the card remains hoverable while
//! the pointer travels from its trigger, and the visible material belongs to
//! the caller. GPUI has no portal, accessibility tree, focusout event, or
//! transform primitive in the pinned version, so those seams are represented
//! as typed metadata and explicit placement/motion plans rather than claimed
//! as platform behavior.

use std::{rc::Rc, time::Duration};

use gpui::{
    Anchor, AnimationExt, AnyElement, App, Bounds, Div, ElementId, Hsla, InteractiveElement,
    IntoElement, MouseButton, ParentElement, Pixels, Point, RenderOnce, SharedString, Size,
    Stateful, StatefulInteractiveElement as _, Styled, Window, anchored, deferred, div, point, px,
    transparent_black,
};

use crate::motion::{MotionAnimation, MotionPlan, MotionPolicy, MotionRecipe};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

/// The zero-delay opening contract reached by the direct `LinkPreview` import.
pub const LINK_PREVIEW_OPEN_DELAY: Duration = Duration::ZERO;
/// The grace period after the pointer leaves an open trigger or card.
pub const LINK_PREVIEW_CLOSE_DELAY: Duration = Duration::from_millis(120);
/// The shared short exit used while a closing presence remains mounted.
pub const LINK_PREVIEW_EXIT_DURATION: Duration = Duration::from_millis(50);
/// The spacing between the trigger and the card on its selected side.
pub const LINK_PREVIEW_SIDE_OFFSET_PX: f32 = 8.0;
/// The reached w-72 card width at the default 16 px root size.
pub const LINK_PREVIEW_WIDTH_PX: f32 = 288.0;
/// The reached max-w-xs width cap at the default 16 px root size.
pub const LINK_PREVIEW_MAX_WIDTH_PX: f32 = 320.0;
/// The deferred paint priority used for this non-modal floating surface.
pub const LINK_PREVIEW_DEFERRED_PRIORITY: usize = 30;

/// Stable debug selectors for the component and its two interactive surfaces.
pub const LINK_PREVIEW_ROOT_SELECTOR: &str = "artisan-link-preview";
/// Stable selector for the caller-owned trigger.
pub const LINK_PREVIEW_TRIGGER_SELECTOR: &str = "artisan-link-preview-trigger";
/// Stable selector for the caller-owned material wrapper.
pub const LINK_PREVIEW_CONTENT_SELECTOR: &str = "artisan-link-preview-content";
/// The description id reached by the context-usage gauge.
pub const LINK_PREVIEW_DESCRIPTION_ID: &str = "context-usage-details";

/// Paint and geometry values for the chrome-stripped `LinkPreview` content shell.
///
/// The shell deliberately has no padding, shadow, or opaque fill. The caller's
/// material, such as `ShaderGlassSurface`, owns the visible surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkPreviewStyle {
    /// The preferred w-72 width.
    pub width: Pixels,
    /// The max-w-xs cap before viewport fitting.
    pub max_width: Pixels,
    /// The reached rounded-2xl radius.
    pub corner_radius: Pixels,
    /// The distance from the trigger on the selected side.
    pub side_offset: Pixels,
    /// Transparent shell fill; caller material supplies the actual paint.
    pub background: Hsla,
    /// Foreground inherited by caller material and text content.
    pub foreground: Hsla,
}

impl LinkPreviewStyle {
    /// Resolves the reached geometry and theme foreground for one mode.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            width: px(LINK_PREVIEW_WIDTH_PX),
            max_width: px(LINK_PREVIEW_MAX_WIDTH_PX),
            corner_radius: RadiusTokens::value(RadiusStep::X2l),
            side_offset: px(LINK_PREVIEW_SIDE_OFFSET_PX),
            background: transparent_black(),
            foreground: theme.colors.foreground.to_paint(),
        }
    }
}

/// Semantic role retained by the native boundary for later accessibility work.
///
/// GPUI 0.2.2 does not emit a platform accessibility tree. These values record
/// the intended roles; they are not a claim that screen readers receive them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkPreviewSemanticRole {
    /// The trigger is expected to be a real caller-owned button.
    Button,
    /// The floating reading surface is a dialog-like hover card.
    Dialog,
}

/// Trigger semantics corresponding to the reached `LinkPreview` attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkPreviewTriggerMetadata {
    /// Intended trigger role.
    pub role: LinkPreviewSemanticRole,
    /// Intended popup role advertised by the trigger.
    pub popup: LinkPreviewSemanticRole,
    /// Whether the controlled content is logically open.
    pub expanded: bool,
    /// Id the trigger intends to control.
    pub controls: SharedString,
    /// Caller-supplied dynamic label, such as Context window 42% full.
    pub label: Option<SharedString>,
    /// Id of the always-mounted caller-supplied description.
    pub described_by: SharedString,
    /// The description is mounted regardless of card visibility.
    pub description_always_mounted: bool,
}

/// Content semantics corresponding to the reached `LinkPreview` content attrs.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkPreviewContentMetadata {
    /// Intended content role.
    pub role: LinkPreviewSemanticRole,
    /// Stable content id used by `LinkPreviewTriggerMetadata::controls`.
    pub id: SharedString,
    /// Current controlled lifecycle phase.
    pub phase: LinkPreviewPhase,
    /// The content contract's tabindex=-1 intent.
    pub tab_index: i32,
    /// Content is not a tab stop in GPUI.
    pub focusable: bool,
    /// The focusout-prevention intent retained from the browser primitive.
    pub focusout_prevented: bool,
    /// The auto-focus-prevention intent retained from the browser primitive.
    pub auto_focus_prevented: bool,
    /// Placement-derived origin, when the caller supplied a placement.
    pub transform_origin: Option<LinkPreviewTransformOrigin>,
}

/// Lifecycle phases used by the clock-injected state machine and renderer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkPreviewPhase {
    /// No card or presence node is mounted.
    Closed,
    /// The zero-delay open transition is pending or entering.
    Opening,
    /// The card is settled open.
    Open,
    /// The card remains mounted while its short exit plays.
    Closing,
}

impl LinkPreviewPhase {
    /// Returns the semantic motion recipe associated with this phase.
    #[must_use]
    pub const fn recipe(self) -> Option<MotionRecipe> {
        match self {
            Self::Opening => Some(MotionRecipe::TooltipIn),
            Self::Closing => Some(MotionRecipe::TooltipOut),
            Self::Closed | Self::Open => None,
        }
    }

    /// Returns the settled target represented by this phase.
    #[must_use]
    pub const fn target_open(self) -> bool {
        matches!(self, Self::Opening | Self::Open)
    }

    /// Returns whether the visible card should be mounted for this phase.
    #[must_use]
    pub const fn is_content_mounted(self) -> bool {
        !matches!(self, Self::Closed)
    }

    /// Returns whether the trigger's expanded state is logically open.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open)
    }

    /// Resolves this phase directly to a settled lifecycle phase.
    #[must_use]
    pub const fn settled(self) -> Self {
        if self.target_open() {
            Self::Open
        } else {
            Self::Closed
        }
    }
}

/// A single deterministic event accepted by `LinkPreviewState`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LinkPreviewEvent {
    /// The pointer entered the caller-owned trigger.
    TriggerEnter,
    /// The pointer left the trigger.
    TriggerLeave,
    /// Focus was observed; only a true focus-visible value opens the card.
    TriggerFocus { focus_visible: bool },
    /// The trigger lost focus.
    TriggerBlur,
    /// The pointer entered the caller-owned content.
    ContentEnter,
    /// The pointer left the content.
    ContentLeave,
    /// Pointer interaction began inside the content.
    ContentPointerDown,
    /// Pointer interaction ended inside the content.
    ContentPointerUp,
    /// The caller's selection state changed inside the content.
    SelectionChanged { contains_selection: bool },
    /// A measured pointer position used by the safe-polygon decision.
    PointerMoved {
        /// Current pointer position in window coordinates.
        pointer: Point<Pixels>,
        /// Current trigger bounds in window coordinates.
        trigger_bounds: Bounds<Pixels>,
        /// Current content bounds in window coordinates.
        content_bounds: Bounds<Pixels>,
    },
    /// Escape requested close.
    Escape,
    /// An outside interaction requested close.
    Outside,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingTransition {
    Open(Duration),
    Close { deadline: Duration, force: bool },
    FinishClose(Duration),
}

/// Pointer hover and focus-visible flags kept out of the state struct lint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct PointerHover {
    trigger_hovered: bool,
    content_hovered: bool,
    trigger_focused: bool,
}

/// Clock-injected `LinkPreview` lifecycle state.
///
/// The state machine never reads a wall clock and never sleeps. Callers pass
/// the time of an event to `LinkPreviewState::transition` and advance pending
/// work with `LinkPreviewState::advance` or `advance_with_policy`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkPreviewState {
    phase: LinkPreviewPhase,
    pending: Option<PendingTransition>,
    hover: PointerHover,
    pointer_down_on_content: bool,
    contains_selection: bool,
    clock: Duration,
    generation: u64,
}

impl Default for LinkPreviewState {
    fn default() -> Self {
        Self::new(false)
    }
}

impl LinkPreviewState {
    /// Creates a settled state at time zero.
    #[must_use]
    pub const fn new(open: bool) -> Self {
        Self {
            phase: if open {
                LinkPreviewPhase::Open
            } else {
                LinkPreviewPhase::Closed
            },
            pending: None,
            hover: PointerHover {
                trigger_hovered: false,
                content_hovered: false,
                trigger_focused: false,
            },
            pointer_down_on_content: false,
            contains_selection: false,
            clock: Duration::ZERO,
            generation: 0,
        }
    }

    /// Applies one event at the supplied monotonic clock value.
    #[must_use]
    pub fn transition(mut self, event: LinkPreviewEvent, at: Duration) -> Self {
        self.set_clock(at);

        match event {
            LinkPreviewEvent::TriggerEnter => {
                self.hover.trigger_hovered = true;
                self.request_open();
            }
            LinkPreviewEvent::TriggerLeave => {
                self.hover.trigger_hovered = false;
                if !self.hover.trigger_focused && !self.hover.content_hovered {
                    self.request_close_or_immediate();
                }
            }
            LinkPreviewEvent::TriggerFocus { focus_visible } => {
                self.hover.trigger_focused = focus_visible;
                if focus_visible {
                    self.request_open();
                } else {
                    self.request_close_if_allowed();
                }
            }
            LinkPreviewEvent::TriggerBlur => {
                self.hover.trigger_focused = false;
                self.request_close_if_allowed();
            }
            LinkPreviewEvent::ContentEnter => {
                self.hover.content_hovered = true;
                self.cancel_close();
                if matches!(self.phase, LinkPreviewPhase::Closed) {
                    self.request_open();
                }
            }
            LinkPreviewEvent::ContentLeave => {
                self.hover.content_hovered = false;
                if !self.hover.trigger_focused && !self.hover.trigger_hovered {
                    self.request_close_if_allowed();
                }
            }
            LinkPreviewEvent::ContentPointerDown => {
                self.pointer_down_on_content = true;
                self.cancel_close();
            }
            LinkPreviewEvent::ContentPointerUp => {
                self.pointer_down_on_content = false;
                self.request_close_if_allowed();
            }
            LinkPreviewEvent::SelectionChanged { contains_selection } => {
                self.contains_selection = contains_selection;
                if contains_selection {
                    self.cancel_close();
                } else {
                    self.request_close_if_allowed();
                }
            }
            LinkPreviewEvent::PointerMoved {
                pointer,
                trigger_bounds,
                content_bounds,
            } => self.handle_pointer_move(pointer, trigger_bounds, content_bounds),
            LinkPreviewEvent::Escape | LinkPreviewEvent::Outside => {
                self.request_explicit_close();
            }
        }

        self
    }

    /// Advances all transitions whose deadlines are at or before now.
    #[must_use]
    pub fn advance(self, now: Duration) -> Self {
        self.advance_with_policy(now, MotionPolicy::Full)
    }

    /// Advances transitions using the supplied full/reduced motion policy.
    #[must_use]
    pub fn advance_with_policy(mut self, now: Duration, motion_policy: MotionPolicy) -> Self {
        self.set_clock(now);

        while let Some(pending) = self.pending {
            match pending {
                PendingTransition::Open(deadline) if deadline <= self.clock => {
                    self.pending = None;
                    if matches!(self.phase, LinkPreviewPhase::Opening) {
                        self.phase = LinkPreviewPhase::Open;
                        self.generation = self.generation.saturating_add(1);
                    }
                }
                PendingTransition::Close { deadline, force } if deadline <= self.clock => {
                    self.pending = None;
                    if matches!(self.phase, LinkPreviewPhase::Open) && (force || !self.keep_alive())
                    {
                        self.phase = LinkPreviewPhase::Closing;
                        self.generation = self.generation.saturating_add(1);
                        let exit_duration = match motion_policy {
                            MotionPolicy::Full => LINK_PREVIEW_EXIT_DURATION,
                            MotionPolicy::Reduced => Duration::ZERO,
                        };
                        self.pending = Some(PendingTransition::FinishClose(
                            self.clock.saturating_add(exit_duration),
                        ));
                    }
                }
                PendingTransition::FinishClose(deadline) if deadline <= self.clock => {
                    self.pending = None;
                    if matches!(self.phase, LinkPreviewPhase::Closing) {
                        if self.keep_alive() {
                            self.phase = LinkPreviewPhase::Open;
                        } else {
                            self.phase = LinkPreviewPhase::Closed;
                        }
                        self.generation = self.generation.saturating_add(1);
                    }
                }
                _ => break,
            }
        }

        self
    }

    /// Settles the current target without changing the transition generation.
    #[must_use]
    pub const fn settle(self) -> Self {
        Self {
            phase: self.phase.settled(),
            pending: None,
            ..self
        }
    }

    /// Current lifecycle phase.
    #[must_use]
    pub const fn phase(self) -> LinkPreviewPhase {
        self.phase
    }

    /// Current monotonic clock value.
    #[must_use]
    pub const fn clock(self) -> Duration {
        self.clock
    }

    /// Current generation for stable animation identity.
    #[must_use]
    pub const fn generation(self) -> u64 {
        self.generation
    }

    /// Whether the card is logically settled open.
    #[must_use]
    pub const fn is_open(self) -> bool {
        self.phase.is_open()
    }

    /// Whether a card or exit-presence node should be mounted.
    #[must_use]
    pub const fn is_content_mounted(self) -> bool {
        self.phase.is_content_mounted()
    }

    /// Whether the current phase has no pending timer or exit.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(
            self.phase,
            LinkPreviewPhase::Closed | LinkPreviewPhase::Open
        ) && self.pending.is_none()
    }

    /// Whether the trigger is currently pointer-hovered.
    #[must_use]
    pub const fn trigger_hovered(self) -> bool {
        self.hover.trigger_hovered
    }

    /// Whether the content is currently pointer-hovered.
    #[must_use]
    pub const fn content_hovered(self) -> bool {
        self.hover.content_hovered
    }

    /// Whether the trigger has focus-visible intent.
    #[must_use]
    pub const fn trigger_focused(self) -> bool {
        self.hover.trigger_focused
    }

    /// Whether pointer-down on content currently suppresses close.
    #[must_use]
    pub const fn pointer_down_on_content(self) -> bool {
        self.pointer_down_on_content
    }

    /// Whether text selection currently suppresses close.
    #[must_use]
    pub const fn contains_selection(self) -> bool {
        self.contains_selection
    }

    /// Returns the next pending deadline, regardless of its phase.
    #[must_use]
    pub const fn next_deadline(self) -> Option<Duration> {
        match self.pending {
            Some(
                PendingTransition::Open(deadline)
                | PendingTransition::Close { deadline, .. }
                | PendingTransition::FinishClose(deadline),
            ) => Some(deadline),
            None => None,
        }
    }

    /// Returns the pending zero-delay open deadline, if present.
    #[must_use]
    pub const fn open_deadline(self) -> Option<Duration> {
        match self.pending {
            Some(PendingTransition::Open(deadline)) => Some(deadline),
            _ => None,
        }
    }

    /// Returns the pending 120 ms close deadline, if present.
    #[must_use]
    pub const fn close_deadline(self) -> Option<Duration> {
        match self.pending {
            Some(PendingTransition::Close { deadline, .. }) => Some(deadline),
            _ => None,
        }
    }

    /// Returns the pending exit-presence deadline, if present.
    #[must_use]
    pub const fn finish_close_deadline(self) -> Option<Duration> {
        match self.pending {
            Some(PendingTransition::FinishClose(deadline)) => Some(deadline),
            _ => None,
        }
    }

    fn set_clock(&mut self, at: Duration) {
        if at > self.clock {
            self.clock = at;
        }
    }

    fn request_open(&mut self) {
        self.cancel_close();
        match self.phase {
            LinkPreviewPhase::Closed => {
                self.phase = LinkPreviewPhase::Opening;
                self.generation = self.generation.saturating_add(1);
                self.pending = Some(PendingTransition::Open(
                    self.clock.saturating_add(LINK_PREVIEW_OPEN_DELAY),
                ));
            }
            LinkPreviewPhase::Opening => {
                if self.open_deadline().is_none() {
                    self.pending = Some(PendingTransition::Open(
                        self.clock.saturating_add(LINK_PREVIEW_OPEN_DELAY),
                    ));
                }
            }
            LinkPreviewPhase::Open | LinkPreviewPhase::Closing => {}
        }
    }

    fn request_close_or_immediate(&mut self) {
        if matches!(self.phase, LinkPreviewPhase::Opening) {
            self.immediate_close();
        } else {
            self.request_close_if_allowed();
        }
    }

    fn request_close_if_allowed(&mut self) {
        if self.pointer_down_on_content || self.contains_selection || self.keep_alive() {
            self.cancel_close();
        } else {
            self.request_close(false);
        }
    }

    fn request_explicit_close(&mut self) {
        if self.pointer_down_on_content || self.contains_selection {
            self.cancel_close();
        } else {
            self.request_close(true);
        }
    }

    fn request_close(&mut self, force: bool) {
        match self.phase {
            LinkPreviewPhase::Closed => self.pending = None,
            LinkPreviewPhase::Opening => self.immediate_close(),
            LinkPreviewPhase::Open => {
                if let Some(PendingTransition::Close {
                    deadline,
                    force: existing_force,
                }) = self.pending
                {
                    if force && !existing_force {
                        self.pending = Some(PendingTransition::Close {
                            deadline,
                            force: true,
                        });
                    }
                } else {
                    self.pending = Some(PendingTransition::Close {
                        deadline: self.clock.saturating_add(LINK_PREVIEW_CLOSE_DELAY),
                        force,
                    });
                }
            }
            LinkPreviewPhase::Closing => {}
        }
    }

    fn immediate_close(&mut self) {
        self.pending = None;
        if !matches!(self.phase, LinkPreviewPhase::Closed) {
            self.phase = LinkPreviewPhase::Closed;
            self.generation = self.generation.saturating_add(1);
        }
    }

    fn cancel_close(&mut self) {
        let had_close = matches!(
            self.pending,
            Some(PendingTransition::Close { .. } | PendingTransition::FinishClose(_))
        );
        if matches!(self.phase, LinkPreviewPhase::Closing) {
            self.phase = LinkPreviewPhase::Open;
            self.generation = self.generation.saturating_add(1);
        }
        if had_close {
            self.pending = None;
        }
    }

    fn keep_alive(&self) -> bool {
        self.hover.trigger_hovered
            || self.hover.content_hovered
            || self.hover.trigger_focused
            || self.pointer_down_on_content
            || self.contains_selection
    }

    fn handle_pointer_move(
        &mut self,
        pointer: Point<Pixels>,
        trigger_bounds: Bounds<Pixels>,
        content_bounds: Bounds<Pixels>,
    ) {
        match pointer_transit(trigger_bounds, content_bounds, pointer) {
            LinkPreviewPointerTransit::Trigger => {
                self.hover.trigger_hovered = true;
                self.hover.content_hovered = false;
                self.request_open();
            }
            LinkPreviewPointerTransit::Content => {
                self.hover.trigger_hovered = false;
                self.hover.content_hovered = true;
                self.cancel_close();
                if matches!(self.phase, LinkPreviewPhase::Closed) {
                    self.request_open();
                }
            }
            LinkPreviewPointerTransit::Grace => {
                self.hover.trigger_hovered = false;
                self.hover.content_hovered = false;
                if matches!(self.phase, LinkPreviewPhase::Opening) && !self.hover.trigger_focused {
                    self.immediate_close();
                } else if !self.hover.trigger_focused {
                    self.cancel_close();
                }
            }
            LinkPreviewPointerTransit::Outside => {
                self.hover.trigger_hovered = false;
                self.hover.content_hovered = false;
                if !self.hover.trigger_focused {
                    self.request_close_if_allowed();
                }
            }
        }
    }
}
/// The area occupied by the two caller-owned `LinkPreview` surfaces.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkPreviewArea {
    /// The trigger rectangle.
    Trigger,
    /// The content rectangle.
    Content,
}

/// Interaction callbacks emitted by the `GPUI` wrappers.
#[derive(Clone, Debug, PartialEq)]
pub enum LinkPreviewInteraction {
    /// Pointer hover changed on the trigger.
    TriggerHover(bool),
    /// Pointer hover changed on the content.
    ContentHover(bool),
    /// Pointer interaction began on content.
    ContentPointerDown,
    /// Pointer interaction ended on content.
    ContentPointerUp,
    /// A pointer move that the owner can feed to `LinkPreviewEvent::PointerMoved`.
    PointerMoved {
        /// The GPUI wrapper that received the move.
        area: LinkPreviewArea,
        /// Window-coordinate pointer position.
        position: Point<Pixels>,
    },
}

/// Classification of a pointer position relative to the trigger, content,
/// and the convex safe hull joining them.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkPreviewPointerTransit {
    /// The pointer is inside the trigger rectangle.
    Trigger,
    /// The pointer is inside the content rectangle.
    Content,
    /// The pointer is in the grace hull between the two rectangles.
    Grace,
    /// The pointer has left the safe hull.
    Outside,
}

impl LinkPreviewPointerTransit {
    /// Whether this classification should keep the hover card alive.
    #[must_use]
    pub const fn keeps_open(self) -> bool {
        !matches!(self, Self::Outside)
    }
}

/// Returns the deterministic safe-polygon classification for one pointer.
#[must_use]
pub fn pointer_transit(
    trigger_bounds: Bounds<Pixels>,
    content_bounds: Bounds<Pixels>,
    pointer: Point<Pixels>,
) -> LinkPreviewPointerTransit {
    if bounds_contains(trigger_bounds, pointer) {
        return LinkPreviewPointerTransit::Trigger;
    }
    if bounds_contains(content_bounds, pointer) {
        return LinkPreviewPointerTransit::Content;
    }
    if safe_polygon_contains(trigger_bounds, content_bounds, pointer) {
        LinkPreviewPointerTransit::Grace
    } else {
        LinkPreviewPointerTransit::Outside
    }
}

/// Returns whether a pointer lies in the trigger/content safe hull.
#[must_use]
pub fn safe_polygon_contains(
    trigger_bounds: Bounds<Pixels>,
    content_bounds: Bounds<Pixels>,
    pointer: Point<Pixels>,
) -> bool {
    let mut points = Vec::with_capacity(8);
    points.extend(bounds_corners(trigger_bounds));
    points.extend(bounds_corners(content_bounds));
    let hull = convex_hull(points);
    point_in_polygon(&hull, GeometryPoint::from(pointer))
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GeometryPoint {
    x: f32,
    y: f32,
}

impl From<Point<Pixels>> for GeometryPoint {
    fn from(point: Point<Pixels>) -> Self {
        Self {
            x: f32::from(point.x),
            y: f32::from(point.y),
        }
    }
}

fn bounds_corners(bounds: Bounds<Pixels>) -> [GeometryPoint; 4] {
    let x = f32::from(bounds.origin.x);
    let y = f32::from(bounds.origin.y);
    let right = x + f32::from(bounds.size.width);
    let bottom = y + f32::from(bounds.size.height);
    [
        GeometryPoint { x, y },
        GeometryPoint { x: right, y },
        GeometryPoint {
            x: right,
            y: bottom,
        },
        GeometryPoint { x, y: bottom },
    ]
}

fn bounds_contains(bounds: Bounds<Pixels>, point: Point<Pixels>) -> bool {
    let x = f32::from(point.x);
    let y = f32::from(point.y);
    let corners = bounds_corners(bounds);
    x >= corners[0].x && x <= corners[1].x && y >= corners[0].y && y <= corners[2].y
}

fn convex_hull(mut points: Vec<GeometryPoint>) -> Vec<GeometryPoint> {
    points.sort_by(|left, right| {
        left.x
            .total_cmp(&right.x)
            .then_with(|| left.y.total_cmp(&right.y))
    });
    points
        .dedup_by(|left, right| (left.x - right.x).abs() < 1e-6 && (left.y - right.y).abs() < 1e-6);
    if points.len() <= 1 {
        return points;
    }

    let mut lower = Vec::with_capacity(points.len());
    for point in points.iter().copied() {
        while lower.len() >= 2
            && cross(lower[lower.len() - 2], lower[lower.len() - 1], point) <= 0.0
        {
            lower.pop();
        }
        lower.push(point);
    }

    let mut upper = Vec::with_capacity(points.len());
    for point in points.iter().rev().copied() {
        while upper.len() >= 2
            && cross(upper[upper.len() - 2], upper[upper.len() - 1], point) <= 0.0
        {
            upper.pop();
        }
        upper.push(point);
    }

    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

fn cross(first: GeometryPoint, second: GeometryPoint, third: GeometryPoint) -> f32 {
    (second.x - first.x) * (third.y - first.y) - (second.y - first.y) * (third.x - first.x)
}

fn point_in_polygon(polygon: &[GeometryPoint], point: GeometryPoint) -> bool {
    match polygon.len() {
        0 => false,
        1 => nearly_equal(polygon[0], point),
        2 => on_segment(polygon[0], polygon[1], point),
        _ => {
            let mut inside = false;
            for index in 0..polygon.len() {
                let first = polygon[index];
                let second = polygon[(index + 1) % polygon.len()];
                if on_segment(first, second, point) {
                    return true;
                }
                if (first.y > point.y) != (second.y > point.y) {
                    let x_at_y =
                        first.x + (point.y - first.y) * (second.x - first.x) / (second.y - first.y);
                    if point.x < x_at_y {
                        inside = !inside;
                    }
                }
            }
            inside
        }
    }
}

fn on_segment(first: GeometryPoint, second: GeometryPoint, point: GeometryPoint) -> bool {
    cross(first, second, point).abs() <= 0.0001
        && point.x >= first.x.min(second.x) - 0.0001
        && point.x <= first.x.max(second.x) + 0.0001
        && point.y >= first.y.min(second.y) - 0.0001
        && point.y <= first.y.max(second.y) + 0.0001
}

fn nearly_equal(first: GeometryPoint, second: GeometryPoint) -> bool {
    (first.x - second.x).abs() <= 0.0001 && (first.y - second.y).abs() <= 0.0001
}

/// The side selected by deterministic placement collision handling.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkPreviewSide {
    /// Preferred side: above the trigger.
    Top,
    /// Collision fallback side: below the trigger.
    Bottom,
}

/// Horizontal alignment retained by the placement contract.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkPreviewAlign {
    /// Align the content's leading edge with the trigger's leading edge.
    Start,
    /// Available for a future right-to-left placement policy.
    End,
}

/// Typed transform-origin intent derived from the selected placement side.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LinkPreviewTransformOrigin {
    /// The content's top-left corner.
    TopStart,
    /// The content's top-right corner.
    TopEnd,
    /// The content's bottom-left corner.
    BottomStart,
    /// The content's bottom-right corner.
    BottomEnd,
}

/// Final window-coordinate placement for one measured trigger and content.
///
/// The caller supplies the measured content size because GPUI's anchored
/// element does not expose a browser-style floating middleware pipeline. The
/// resolver chooses top/start first, flips to bottom when top collides, and
/// clamps the result into the supplied viewport deterministically.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkPreviewPlacement {
    /// Window-coordinate top-left origin.
    pub origin: Point<Pixels>,
    /// Natural content size used during collision resolution.
    pub size: Size<Pixels>,
    /// Selected vertical side.
    pub side: LinkPreviewSide,
    /// Selected horizontal alignment.
    pub align: LinkPreviewAlign,
    /// Offset retained in the final decision.
    pub side_offset: Pixels,
    /// Origin used by the presence animation policy.
    pub transform_origin: LinkPreviewTransformOrigin,
    /// Whether a flip or clamp changed the preferred top/start result.
    pub collision_adjusted: bool,
}

impl LinkPreviewPlacement {
    /// Resolves top/start placement with the fixed 8 px side offset.
    #[must_use]
    pub fn resolve(
        trigger_bounds: Bounds<Pixels>,
        content_size: Size<Pixels>,
        viewport_size: Size<Pixels>,
    ) -> Self {
        Self::resolve_with_offset(
            trigger_bounds,
            content_size,
            viewport_size,
            px(LINK_PREVIEW_SIDE_OFFSET_PX),
        )
    }

    /// Resolves top/start placement with an explicit offset for style reuse.
    #[must_use]
    pub fn resolve_with_offset(
        trigger_bounds: Bounds<Pixels>,
        content_size: Size<Pixels>,
        viewport_size: Size<Pixels>,
        side_offset: Pixels,
    ) -> Self {
        let trigger_x = f32::from(trigger_bounds.origin.x);
        let trigger_top = f32::from(trigger_bounds.origin.y);
        let trigger_bottom = trigger_top + f32::from(trigger_bounds.size.height);
        let content_width = f32::from(content_size.width);
        let content_height = f32::from(content_size.height);
        let viewport_width = f32::from(viewport_size.width).max(0.0);
        let viewport_height = f32::from(viewport_size.height).max(0.0);
        let offset = f32::from(side_offset).max(0.0);

        let top_y = trigger_top - content_height - offset;
        let bottom_y = trigger_bottom + offset;
        let top_fits = top_y >= 0.0;
        let bottom_fits = bottom_y + content_height <= viewport_height;
        let (side, desired_y) = if top_fits {
            (LinkPreviewSide::Top, top_y)
        } else if bottom_fits {
            (LinkPreviewSide::Bottom, bottom_y)
        } else {
            (LinkPreviewSide::Top, top_y)
        };

        let max_x = (viewport_width - content_width).max(0.0);
        let max_y = (viewport_height - content_height).max(0.0);
        let origin_x = clamp_origin(trigger_x, max_x);
        let origin_y = clamp_origin(desired_y, max_y);
        let collision_adjusted = side == LinkPreviewSide::Bottom
            || (origin_x - trigger_x).abs() > 0.0001
            || (origin_y - desired_y).abs() > 0.0001;
        let transform_origin = match side {
            LinkPreviewSide::Top => LinkPreviewTransformOrigin::BottomStart,
            LinkPreviewSide::Bottom => LinkPreviewTransformOrigin::TopStart,
        };

        Self {
            origin: point(px(origin_x), px(origin_y)),
            size: content_size,
            side,
            align: LinkPreviewAlign::Start,
            side_offset,
            transform_origin,
            collision_adjusted,
        }
    }

    /// Returns the final content bounds.
    #[must_use]
    pub fn bounds(self) -> Bounds<Pixels> {
        Bounds::new(self.origin, self.size)
    }

    /// Whether collision handling changed the preferred result.
    #[must_use]
    pub const fn was_collision_adjusted(self) -> bool {
        self.collision_adjusted
    }
}

fn clamp_origin(value: f32, maximum: f32) -> f32 {
    value.max(0.0).min(maximum)
}
/// The `GPUI` support decision for one `LinkPreview` visual effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkPreviewEffectPlan {
    /// Resolve the effect at its final value without an animation.
    Immediate,
    /// Animate the effect using the shared GPUI clock.
    Animated,
    /// GPUI 0.2.2 has no corresponding visual primitive.
    UnsupportedByGpui,
}

/// The resolved motion plan for one `LinkPreview` phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinkPreviewMotionPlan {
    /// Requested phase before reduced-motion settling.
    pub phase: LinkPreviewPhase,
    /// Shared full/reduced motion decision.
    pub motion: MotionPlan,
    /// Opacity is supported by GPUI's animation wrapper.
    pub opacity: LinkPreviewEffectPlan,
    /// The audited entrance scale/transform is unavailable in GPUI 0.2.2.
    pub transform: LinkPreviewEffectPlan,
}

impl LinkPreviewMotionPlan {
    /// Resolves the requested phase under an explicit motion policy.
    #[must_use]
    pub fn for_phase(phase: LinkPreviewPhase, motion_policy: MotionPolicy) -> Self {
        let motion = phase.recipe().map_or(MotionPlan::Immediate, |recipe| {
            motion_policy.resolve(recipe)
        });
        let opacity = if motion.animation().is_some() {
            LinkPreviewEffectPlan::Animated
        } else {
            LinkPreviewEffectPlan::Immediate
        };

        Self {
            phase,
            motion,
            opacity,
            transform: LinkPreviewEffectPlan::UnsupportedByGpui,
        }
    }

    /// Returns the phase actually rendered after reduced-motion settling.
    #[must_use]
    pub const fn effective_phase(self) -> LinkPreviewPhase {
        match self.motion {
            MotionPlan::Immediate => self.phase.settled(),
            MotionPlan::Animate(_) => self.phase,
        }
    }

    /// Returns the shared recipe selected by the requested phase.
    #[must_use]
    pub const fn recipe(self) -> Option<MotionRecipe> {
        self.phase.recipe()
    }

    /// Returns the shared animation specification, when full motion is active.
    #[must_use]
    pub const fn animation(self) -> Option<MotionAnimation> {
        self.motion.animation()
    }

    /// Whether the content or exit-presence node is mounted.
    #[must_use]
    pub const fn content_present(self) -> bool {
        self.effective_phase().is_content_mounted()
    }
}

/// A callback installed on the trigger and content wrappers.
pub type LinkPreviewInteractionHandler = Rc<dyn Fn(LinkPreviewInteraction, &mut Window, &mut App)>;

/// A controlled native `LinkPreview` component.
///
/// The caller owns trigger semantics, description text, content/material, and
/// the measured placement. This component supplies hover event forwarding,
/// deterministic presence mounting, transparent content chrome, and GPUI's
/// deferred anchored paint. It intentionally does not claim browser
/// accessibility, focusout cancellation, or transform-origin styling that the
/// pinned GPUI version cannot represent.
#[derive(IntoElement)]
pub struct LinkPreview {
    root: Div,
    id: ElementId,
    trigger: Div,
    description: AnyElement,
    content: AnyElement,
    description_id: SharedString,
    content_id: SharedString,
    trigger_label: Option<SharedString>,
    style: LinkPreviewStyle,
    state: LinkPreviewState,
    motion_policy: MotionPolicy,
    placement: Option<LinkPreviewPlacement>,
    debug_selector: Option<SharedString>,
    on_interaction: Option<LinkPreviewInteractionHandler>,
}
/// Attaches hover and pointer-move forwarding to the stateful trigger shell.
fn wire_trigger_interactions(
    mut trigger: Stateful<Div>,
    on_interaction: Option<&LinkPreviewInteractionHandler>,
) -> Stateful<Div> {
    if let Some(handler) = on_interaction.as_ref() {
        let handler = Rc::clone(handler);
        trigger = trigger.on_hover(move |hovered, window, cx| {
            handler(LinkPreviewInteraction::TriggerHover(*hovered), window, cx);
        });
    }
    if let Some(handler) = on_interaction.as_ref() {
        let handler = Rc::clone(handler);
        trigger = trigger.on_mouse_move(move |event, window, cx| {
            handler(
                LinkPreviewInteraction::PointerMoved {
                    area: LinkPreviewArea::Trigger,
                    position: event.position,
                },
                window,
                cx,
            );
        });
    }
    trigger
}

/// Attaches hover, press, release, and pointer-move forwarding to the shell.
fn wire_content_interactions(
    mut content_shell: Stateful<Div>,
    on_interaction: Option<&LinkPreviewInteractionHandler>,
) -> Stateful<Div> {
    if let Some(handler) = on_interaction.as_ref() {
        let handler = Rc::clone(handler);
        content_shell = content_shell.on_hover(move |hovered, window, cx| {
            handler(LinkPreviewInteraction::ContentHover(*hovered), window, cx);
        });
    }
    if let Some(handler) = on_interaction.as_ref() {
        let handler = Rc::clone(handler);
        content_shell = content_shell.on_any_mouse_down(move |_, window, cx| {
            handler(LinkPreviewInteraction::ContentPointerDown, window, cx);
        });
    }
    if let Some(handler) = on_interaction.as_ref() {
        let handler = Rc::clone(handler);
        content_shell = content_shell.on_mouse_up(MouseButton::Left, move |_, window, cx| {
            handler(LinkPreviewInteraction::ContentPointerUp, window, cx);
        });
    }
    if let Some(handler) = on_interaction.as_ref() {
        let handler = Rc::clone(handler);
        content_shell = content_shell.on_mouse_move(move |event, window, cx| {
            handler(
                LinkPreviewInteraction::PointerMoved {
                    area: LinkPreviewArea::Content,
                    position: event.position,
                },
                window,
                cx,
            );
        });
    }
    content_shell
}

impl LinkPreview {
    /// Constructs a controlled preview with a default always-mounted
    /// description. Replace it with the caller's dynamic description using
    /// `LinkPreview::description`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        trigger: Div,
        content: impl IntoElement,
        style: LinkPreviewStyle,
        state: LinkPreviewState,
    ) -> Self {
        let id = id.into();
        let content_id: SharedString = format!("{id}-content").into();
        Self {
            root: div().relative().flex().flex_col().min_w_0(),
            id,
            trigger,
            description: div()
                .child(SharedString::from("Link preview details"))
                .into_any_element(),
            content: content.into_any_element(),
            description_id: LINK_PREVIEW_DESCRIPTION_ID.into(),
            content_id,
            trigger_label: None,
            style,
            state,
            motion_policy: MotionPolicy::Full,
            placement: None,
            debug_selector: None,
            on_interaction: None,
        }
    }

    /// Constructs a preview after resolving the shared theme tokens.
    #[must_use]
    pub fn from_theme(
        id: impl Into<ElementId>,
        trigger: Div,
        content: impl IntoElement,
        theme: ArtisanTheme,
        state: LinkPreviewState,
    ) -> Self {
        Self::new(
            id,
            trigger,
            content,
            LinkPreviewStyle::resolve(theme),
            state,
        )
    }

    /// Replaces the always-mounted caller-supplied description element.
    #[must_use]
    pub fn description(mut self, description: impl IntoElement) -> Self {
        self.description = description.into_any_element();
        self
    }

    /// Overrides the description id retained in trigger metadata.
    #[must_use]
    pub fn description_id(mut self, description_id: impl Into<SharedString>) -> Self {
        self.description_id = description_id.into();
        self
    }

    /// Overrides the generated content id retained in trigger metadata.
    #[must_use]
    pub fn content_id(mut self, content_id: impl Into<SharedString>) -> Self {
        self.content_id = content_id.into();
        self
    }

    /// Supplies the dynamic trigger label used by semantic metadata.
    #[must_use]
    pub fn trigger_label(mut self, label: impl Into<SharedString>) -> Self {
        self.trigger_label = Some(label.into());
        self
    }

    /// Applies an explicit state snapshot to this render.
    #[must_use]
    pub const fn with_state(mut self, state: LinkPreviewState) -> Self {
        self.state = state;
        self
    }

    /// Selects the explicit full/reduced motion policy for this render.
    #[must_use]
    pub const fn motion_policy(mut self, motion_policy: MotionPolicy) -> Self {
        self.motion_policy = motion_policy;
        self
    }

    /// Supplies a previously resolved window-coordinate placement.
    #[must_use]
    pub const fn placement(mut self, placement: LinkPreviewPlacement) -> Self {
        self.placement = Some(placement);
        self
    }

    /// Resolves and supplies placement from measured window-coordinate bounds.
    #[must_use]
    pub fn placement_from_bounds(
        mut self,
        trigger_bounds: Bounds<Pixels>,
        content_size: Size<Pixels>,
        viewport_size: Size<Pixels>,
    ) -> Self {
        self.placement = Some(LinkPreviewPlacement::resolve_with_offset(
            trigger_bounds,
            content_size,
            viewport_size,
            self.style.side_offset,
        ));
        self
    }

    /// Uses a custom root selector; children receive -trigger and -content.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Installs the callback that forwards GPUI interaction events.
    #[must_use]
    pub fn on_interaction(
        mut self,
        handler: impl Fn(LinkPreviewInteraction, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_interaction = Some(Rc::new(handler));
        self
    }

    /// Current controlled state.
    #[must_use]
    pub const fn state(&self) -> LinkPreviewState {
        self.state
    }

    /// Current requested phase.
    #[must_use]
    pub const fn phase(&self) -> LinkPreviewPhase {
        self.state.phase()
    }

    /// Resolved style retained by the render recipe.
    #[must_use]
    pub const fn visual_style(&self) -> LinkPreviewStyle {
        self.style
    }

    /// Resolved motion plan for the current phase and policy.
    #[must_use]
    pub fn motion_plan(&self) -> LinkPreviewMotionPlan {
        LinkPreviewMotionPlan::for_phase(self.state.phase(), self.motion_policy)
    }

    /// Resolved placement, when the caller supplied one.
    #[must_use]
    pub const fn resolved_placement(&self) -> Option<LinkPreviewPlacement> {
        self.placement
    }
    /// Returns the trigger's retained semantic intent.
    #[must_use]
    pub fn trigger_metadata(&self) -> LinkPreviewTriggerMetadata {
        LinkPreviewTriggerMetadata {
            role: LinkPreviewSemanticRole::Button,
            popup: LinkPreviewSemanticRole::Dialog,
            expanded: self.state.is_open(),
            controls: self.content_id.clone(),
            label: self.trigger_label.clone(),
            described_by: self.description_id.clone(),
            description_always_mounted: true,
        }
    }

    /// Returns the content's retained semantic and focus intent.
    #[must_use]
    pub fn content_metadata(&self) -> LinkPreviewContentMetadata {
        LinkPreviewContentMetadata {
            role: LinkPreviewSemanticRole::Dialog,
            id: self.content_id.clone(),
            phase: self.state.phase(),
            tab_index: -1,
            focusable: false,
            focusout_prevented: true,
            auto_focus_prevented: true,
            transform_origin: self.placement.map(|placement| placement.transform_origin),
        }
    }
}

impl Styled for LinkPreview {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.root.style()
    }
}

impl RenderOnce for LinkPreview {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let plan = LinkPreviewMotionPlan::for_phase(self.state.phase(), self.motion_policy);
        let root_selector = self.debug_selector.as_ref().map_or_else(
            || LINK_PREVIEW_ROOT_SELECTOR.to_string(),
            ToString::to_string,
        );
        let trigger_selector = self.debug_selector.as_ref().map_or_else(
            || LINK_PREVIEW_TRIGGER_SELECTOR.to_string(),
            |selector| format!("{selector}-trigger"),
        );
        let content_selector = self.debug_selector.as_ref().map_or_else(
            || LINK_PREVIEW_CONTENT_SELECTOR.to_string(),
            |selector| format!("{selector}-content"),
        );

        let Self {
            mut root,
            id,
            trigger,
            description,
            content,
            description_id,
            content_id,
            style,
            placement,
            on_interaction,
            state,
            ..
        } = self;

        root = root.debug_selector(move || root_selector);

        let description = div()
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .w(px(1.0))
            .h(px(1.0))
            .overflow_hidden()
            .tab_stop(false)
            .child(description)
            .id(ElementId::Name(description_id));
        root = root.child(description);

        let trigger = wire_trigger_interactions(trigger.id(id), on_interaction.as_ref());
        let trigger = trigger.debug_selector(move || trigger_selector);
        root = root.child(trigger);

        if plan.content_present() {
            let content_shell = wire_content_interactions(
                link_preview_content(style, content)
                    .id(ElementId::Name(content_id))
                    .tab_stop(false)
                    .debug_selector(move || content_selector),
                on_interaction.as_ref(),
            );
            let content = animate_content(content_shell, plan, state.generation());

            if let Some(placement) = placement {
                let anchored_content = anchored()
                    .anchor(Anchor::TopLeft)
                    .position(placement.origin)
                    .snap_to_window()
                    .child(content);
                root = root.child(
                    deferred(anchored_content).with_priority(LINK_PREVIEW_DEFERRED_PRIORITY),
                );
            } else {
                root = root.child(content);
            }
        }

        root
    }
}

/// Builds the transparent, chrome-stripped content shell around caller material.
#[must_use]
pub fn link_preview_content(style: LinkPreviewStyle, material: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .w(style.width)
        .max_w(style.max_width)
        .rounded(style.corner_radius)
        .bg(style.background)
        .text_color(style.foreground)
        .tab_stop(false)
        .child(material)
}

/// Convenience constructor using an already resolved style and state.
#[must_use]
pub fn link_preview(
    id: impl Into<ElementId>,
    trigger: Div,
    content: impl IntoElement,
    style: LinkPreviewStyle,
    state: LinkPreviewState,
) -> LinkPreview {
    LinkPreview::new(id, trigger, content, style, state)
}

/// Samples the shared `SmoothOut` cubic-bezier at `f32` clock precision.
///
/// This mirrors `MotionCurve::sample` for the menu open/close opacity clock so
/// the render path performs no narrowing conversion. Values match the shared
/// solver to float precision and clamp to the exact endpoints.
fn sample_smooth_out(progress: f32) -> f32 {
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }

    let mut lower = 0.0_f32;
    let mut upper = 1.0_f32;
    for _ in 0..48 {
        let time = lower.midpoint(upper);
        if smooth_out_axis(time, 0.22, 0.36) < progress {
            lower = time;
        } else {
            upper = time;
        }
    }
    smooth_out_axis(lower.midpoint(upper), 1.0, 1.0)
}

/// Evaluates one cubic-bezier axis at `f32` precision.
fn smooth_out_axis(time: f32, first: f32, second: f32) -> f32 {
    let inverse = 1.0 - time;
    3.0 * inverse * inverse * time * first
        + 3.0 * inverse * time * time * second
        + time * time * time
}

fn animate_content<E>(content: E, plan: LinkPreviewMotionPlan, generation: u64) -> AnyElement
where
    E: IntoElement + Styled + 'static,
{
    let Some(animation) = plan.animation() else {
        return content.into_any_element();
    };

    let opening = matches!(plan.phase, LinkPreviewPhase::Opening);
    let animation_id = format!(
        "artisan-link-preview-{}-{generation}",
        if opening { "opening" } else { "closing" }
    );
    let initial_opacity = if opening { 0.0 } else { 1.0 };

    // t-tt-presence deliberately has no entrance delay. The zero-delay
    // interaction timer is already represented by LinkPreviewState.
    content
        .opacity(initial_opacity)
        .with_animation(
            ElementId::Name(animation_id.into()),
            animation.gpui_clock(),
            move |content, progress| {
                let eased = sample_smooth_out(progress);
                let opacity = if opening { eased } else { 1.0 - eased };
                content.opacity(opacity.clamp(0.0, 1.0))
            },
        )
        .into_any_element()
}
