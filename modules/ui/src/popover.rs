//! Native anchored popovers for the shared GPUI surface layer.
//!
//! [`Popover`] keeps its open value controlled by the caller and composes a
//! trigger with a content slot. The content is painted in a deferred window
//! layer, while [`resolve_popover_geometry`] remains a pure function that can
//! be tested without a window.
//!
//! The default content recipe is the reached legacy `card bg-popover gap-4
//! rounded-2xl p-4 w-72` shape: a 288 px width, 16 px gap and padding, an
//! 18 px radius, mode-resolved popover colors, and the shared four-layer card
//! elevation. [`PopoverVariant::Bare`] keeps only the shared flex/text
//! foundation so a caller can provide its own material.
//!
//! Pinned GPUI 0.2.2 has no browser ARIA tree, document-level escape-layer
//! stack, focus-scope manager, or presence manager. This primitive therefore
//! exposes real native focus tracking, pointer-outside and Escape callbacks,
//! but does not claim platform accessibility semantics, a focus trap/focus
//! restoration policy, top-most nested-layer arbitration, or animated
//! mount/unmount. Callers own those policies when their surface needs them;
//! existing Artisan motion recipes remain available at that boundary.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::{Refineable as _, Styled};
use gpui::{
    AnyElement, App, Bounds, ClickEvent, Display, Element, ElementId, FocusHandle,
    InteractiveElement as _, IntoElement, KeyDownEvent, LayoutId, ParentElement, Pixels, Point,
    Position, RenderOnce, SharedString, Size, StatefulInteractiveElement as _, Style,
    StyleRefinement, Window, canvas, deferred, div, point, px,
};

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ShadowLayer};

/// How much of the legacy popover content wrapper is supplied by the
/// primitive.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PopoverVariant {
    /// The audited rounded, padded, elevated card surface.
    #[default]
    Default,
    /// The flex/text foundation without card material or fixed width.
    Bare,
}

/// The preferred edge on which popover content is placed.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PopoverSide {
    /// Place content above the trigger.
    Top,
    /// Place content to the right of the trigger.
    Right,
    /// Place content below the trigger.
    #[default]
    Bottom,
    /// Place content to the left of the trigger.
    Left,
}

impl PopoverSide {
    /// Returns the opposite edge used by collision flipping.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Right => Self::Left,
            Self::Bottom => Self::Top,
            Self::Left => Self::Right,
        }
    }

    const fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }
}

/// The alignment of popover content along the trigger's non-side axis.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum PopoverAlign {
    /// Align the content's leading edge with the trigger's leading edge.
    Start,
    /// Center the content on the trigger.
    #[default]
    Center,
    /// Align the content's trailing edge with the trigger's trailing edge.
    End,
}

/// Why a controlled open value was requested.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PopoverChangeReason {
    /// The trigger received a standard pointer or keyboard activation.
    Trigger,
    /// A pointer-down occurred outside the trigger/content boundary.
    Outside,
    /// Escape was pressed while the popover was open.
    Escape,
}

/// The two offset axes used by a floating popover.
///
/// `side` is measured away from the trigger on the selected side. `align` is
/// measured in the positive window axis, independent of side. Negative values
/// are retained so callers can deliberately overlap the trigger.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopoverOffset {
    /// Distance along the side axis.
    pub side: Pixels,
    /// Distance along the alignment axis.
    pub align: Pixels,
}

impl PopoverOffset {
    /// Creates a two-axis offset.
    #[must_use]
    pub const fn new(side: Pixels, align: Pixels) -> Self {
        Self { side, align }
    }
}

/// Placement preferences for an anchored popover.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopoverPlacement {
    /// Preferred side before viewport collision adjustment.
    pub side: PopoverSide,
    /// Alignment along the preferred side.
    pub align: PopoverAlign,
    /// Main-axis separation from the trigger.
    pub side_offset: Pixels,
    /// Cross-axis adjustment.
    pub align_offset: Pixels,
}

impl Default for PopoverPlacement {
    fn default() -> Self {
        Self {
            side: PopoverSide::Bottom,
            align: PopoverAlign::Center,
            side_offset: px(4.0),
            align_offset: px(0.0),
        }
    }
}

impl PopoverPlacement {
    /// Creates placement preferences with explicit side, alignment, and
    /// offsets.
    #[must_use]
    pub const fn new(
        side: PopoverSide,
        align: PopoverAlign,
        side_offset: Pixels,
        align_offset: Pixels,
    ) -> Self {
        Self {
            side,
            align,
            side_offset,
            align_offset,
        }
    }

    /// Replaces the preferred side.
    #[must_use]
    pub const fn side(mut self, side: PopoverSide) -> Self {
        self.side = side;
        self
    }

    /// Replaces the alignment.
    #[must_use]
    pub const fn align(mut self, align: PopoverAlign) -> Self {
        self.align = align;
        self
    }

    /// Replaces the main-axis separation.
    #[must_use]
    pub const fn side_offset(mut self, side_offset: Pixels) -> Self {
        self.side_offset = side_offset;
        self
    }

    /// Replaces the cross-axis adjustment.
    #[must_use]
    pub const fn align_offset(mut self, align_offset: Pixels) -> Self {
        self.align_offset = align_offset;
        self
    }

    /// Replaces both offset axes.
    #[must_use]
    pub const fn offset(mut self, offset: PopoverOffset) -> Self {
        self.side_offset = offset.side;
        self.align_offset = offset.align;
        self
    }

    /// Returns the two offsets as a value object.
    #[must_use]
    pub const fn offsets(self) -> PopoverOffset {
        PopoverOffset::new(self.side_offset, self.align_offset)
    }

    /// Resolves a content rectangle against a viewport.
    ///
    /// The preferred side is flipped only when it cannot fit and the opposite
    /// side can fit, or when the opposite side has strictly more available
    /// main-axis space. The result is then shifted into the viewport on both
    /// axes. If content is larger than the viewport, its origin is
    /// deterministically clamped to zero on that axis.
    #[must_use]
    pub fn resolve(
        self,
        anchor: Bounds<Pixels>,
        content_size: Size<Pixels>,
        viewport_size: Size<Pixels>,
    ) -> PopoverGeometry {
        resolve_popover_geometry(anchor, content_size, viewport_size, self)
    }
}

/// The final, collision-adjusted popover rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PopoverGeometry {
    /// Final top-left origin in window coordinates.
    pub origin: Point<Pixels>,
    /// Measured content size.
    pub size: Size<Pixels>,
    /// Side after possible collision flipping.
    pub side: PopoverSide,
    /// Alignment used for the cross axis.
    pub align: PopoverAlign,
    /// Whether the preferred side was replaced by its opposite.
    pub flipped: bool,
    /// Whether viewport clamping changed either coordinate.
    pub shifted: bool,
}

impl PopoverGeometry {
    /// Returns the final content bounds.
    #[must_use]
    pub fn bounds(self) -> Bounds<Pixels> {
        Bounds::new(self.origin, self.size)
    }

    /// Returns whether collision resolution changed the preferred side.
    #[must_use]
    pub const fn is_flipped(self) -> bool {
        self.flipped
    }

    /// Returns whether collision resolution shifted the rectangle.
    #[must_use]
    pub const fn is_shifted(self) -> bool {
        self.shifted
    }
}

/// Resolves a popover rectangle without depending on GPUI layout or input.
///
/// This is the testable counterpart of the floating layer used by
/// [`Popover`]. Viewport coordinates start at `(0, 0)`, matching
/// [`Window::viewport_size`].
#[must_use]
pub fn resolve_popover_geometry(
    anchor: Bounds<Pixels>,
    content_size: Size<Pixels>,
    viewport_size: Size<Pixels>,
    placement: PopoverPlacement,
) -> PopoverGeometry {
    let preferred_side = placement.side;
    let opposite_side = preferred_side.opposite();
    let main_extent = if preferred_side.is_horizontal() {
        f32::from(content_size.width)
    } else {
        f32::from(content_size.height)
    };
    let preferred_space = available_main_space(
        &anchor,
        viewport_size,
        preferred_side,
        f32::from(placement.side_offset),
    );
    let opposite_space = available_main_space(
        &anchor,
        viewport_size,
        opposite_side,
        f32::from(placement.side_offset),
    );

    let preferred_fits = preferred_space >= main_extent;
    let opposite_fits = opposite_space >= main_extent;
    let side = if !preferred_fits && (opposite_fits || opposite_space > preferred_space) {
        opposite_side
    } else {
        preferred_side
    };

    let desired = preferred_origin(&anchor, content_size, placement, side);
    let (x, shifted_x) = clamp_axis(
        f32::from(desired.x),
        f32::from(content_size.width),
        f32::from(viewport_size.width),
    );
    let (y, shifted_y) = clamp_axis(
        f32::from(desired.y),
        f32::from(content_size.height),
        f32::from(viewport_size.height),
    );

    PopoverGeometry {
        origin: point(px(x), px(y)),
        size: content_size,
        side,
        align: placement.align,
        flipped: side != preferred_side,
        shifted: shifted_x || shifted_y,
    }
}

fn available_main_space(
    anchor: &Bounds<Pixels>,
    viewport_size: Size<Pixels>,
    side: PopoverSide,
    side_offset: f32,
) -> f32 {
    let viewport_width = f32::from(viewport_size.width);
    let viewport_height = f32::from(viewport_size.height);
    match side {
        PopoverSide::Top => f32::from(anchor.top()) - side_offset,
        PopoverSide::Right => viewport_width - f32::from(anchor.right()) - side_offset,
        PopoverSide::Bottom => viewport_height - f32::from(anchor.bottom()) - side_offset,
        PopoverSide::Left => f32::from(anchor.left()) - side_offset,
    }
}

fn preferred_origin(
    anchor: &Bounds<Pixels>,
    content_size: Size<Pixels>,
    placement: PopoverPlacement,
    side: PopoverSide,
) -> Point<Pixels> {
    let anchor_left = f32::from(anchor.left());
    let anchor_right = f32::from(anchor.right());
    let anchor_top = f32::from(anchor.top());
    let anchor_bottom = f32::from(anchor.bottom());
    let content_width = f32::from(content_size.width);
    let content_height = f32::from(content_size.height);
    let side_offset = f32::from(placement.side_offset);
    let align_offset = f32::from(placement.align_offset);

    if side.is_horizontal() {
        let y = aligned_origin(anchor_top, anchor_bottom, content_height, placement.align)
            + align_offset;
        let x = match side {
            PopoverSide::Left => anchor_left - side_offset - content_width,
            PopoverSide::Right => anchor_right + side_offset,
            PopoverSide::Top | PopoverSide::Bottom => unreachable!(),
        };
        point(px(x), px(y))
    } else {
        let x = aligned_origin(anchor_left, anchor_right, content_width, placement.align)
            + align_offset;
        let y = match side {
            PopoverSide::Top => anchor_top - side_offset - content_height,
            PopoverSide::Bottom => anchor_bottom + side_offset,
            PopoverSide::Left | PopoverSide::Right => unreachable!(),
        };
        point(px(x), px(y))
    }
}

fn aligned_origin(start: f32, end: f32, content_extent: f32, align: PopoverAlign) -> f32 {
    match align {
        PopoverAlign::Start => start,
        PopoverAlign::Center => (start + end - content_extent) / 2.0,
        PopoverAlign::End => end - content_extent,
    }
}

fn clamp_axis(value: f32, content_extent: f32, viewport_extent: f32) -> (f32, bool) {
    let maximum = viewport_extent - content_extent;
    let clamped = if maximum < 0.0 {
        0.0
    } else {
        value.clamp(0.0, maximum)
    };
    (clamped, (clamped - value).abs() > f32::EPSILON)
}

/// The mode-resolved content recipe for one popover.
#[derive(Clone, Copy, Debug)]
pub struct PopoverStyle {
    /// Selected material variant.
    pub variant: PopoverVariant,
    /// Fixed default width corresponding to the legacy `w-72` utility.
    pub width: Pixels,
    /// Default card padding (`p-4`).
    pub padding: Pixels,
    /// Default card child gap (`gap-4`).
    pub gap: Pixels,
    /// Default card corner radius (`rounded-2xl`).
    pub corner_radius: Pixels,
    /// Legacy `text-sm` size.
    pub text_size: Pixels,
    /// Legacy `text-sm` line height.
    pub line_height: Pixels,
    /// Mode-resolved `popover` surface.
    pub background: gpui::Hsla,
    /// Mode-resolved `popover-foreground` text.
    pub foreground: gpui::Hsla,
    /// The shared audited four-layer card elevation.
    pub card_shadow: [ShadowLayer; 4],
}

impl PopoverStyle {
    /// Resolves a variant from the shared theme.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, variant: PopoverVariant) -> Self {
        Self {
            variant,
            width: px(288.0),
            padding: theme.spacing.steps(4.0),
            gap: theme.spacing.steps(4.0),
            corner_radius: RadiusTokens::value(RadiusStep::X2l),
            text_size: theme.typography.control_text,
            line_height: px(20.0),
            background: theme.colors.popover.to_paint(),
            foreground: theme.colors.popover_foreground.to_paint(),
            card_shadow: theme.elevation.card_shadow,
        }
    }

    /// Resolves the default card variant.
    #[must_use]
    pub fn default_card(theme: ArtisanTheme) -> Self {
        Self::resolve(theme, PopoverVariant::Default)
    }

    /// Resolves the bare variant.
    #[must_use]
    pub fn bare(theme: ArtisanTheme) -> Self {
        Self::resolve(theme, PopoverVariant::Bare)
    }

    /// Returns whether the card material should be applied.
    #[must_use]
    pub const fn has_card_chrome(self) -> bool {
        matches!(self.variant, PopoverVariant::Default)
    }

    /// Returns whether the caller owns all card material.
    #[must_use]
    pub const fn is_bare(self) -> bool {
        matches!(self.variant, PopoverVariant::Bare)
    }

    /// Converts the shared card elevation into GPUI shadows.
    #[must_use]
    pub fn shadows(self) -> Vec<gpui::BoxShadow> {
        self.card_shadow
            .iter()
            .copied()
            .map(ShadowLayer::to_box_shadow)
            .collect()
    }
}

/// Wraps caller content in the selected popover content recipe.
///
/// The bare variant intentionally does not set width, padding, gap, radius,
/// background, overflow, or elevation.
#[must_use]
pub fn popover_content(style: PopoverStyle, content: impl IntoElement) -> gpui::Div {
    let mut element = div()
        .flex()
        .flex_col()
        .text_size(style.text_size)
        .line_height(style.line_height)
        .text_color(style.foreground)
        .child(content);

    if style.has_card_chrome() {
        element = element
            .gap(style.gap)
            .p(style.padding)
            .w(style.width)
            .rounded(style.corner_radius)
            .overflow_hidden()
            .bg(style.background)
            .shadow(style.shadows());
    }

    element
}

/// Controlled open and disabled state for a [`Popover`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct PopoverState {
    open: bool,
    disabled: bool,
}

impl PopoverState {
    /// Creates state from caller-owned values.
    #[must_use]
    pub const fn new(open: bool, disabled: bool) -> Self {
        Self { open, disabled }
    }

    /// Returns the controlled open value.
    #[must_use]
    pub const fn open(self) -> bool {
        self.open
    }

    /// Alias for [`Self::open`].
    #[must_use]
    pub const fn is_open(self) -> bool {
        self.open
    }

    /// Returns whether trigger activation is disabled.
    #[must_use]
    pub const fn disabled(self) -> bool {
        self.disabled
    }

    /// Alias for [`Self::disabled`].
    #[must_use]
    pub const fn is_disabled(self) -> bool {
        self.disabled
    }

    /// Returns the next open value for a valid trigger activation.
    #[must_use]
    pub const fn requested_toggle(self) -> Option<bool> {
        if self.disabled {
            None
        } else {
            Some(!self.open)
        }
    }

    /// Returns whether an outside or Escape dismissal should be emitted.
    #[must_use]
    pub const fn requests_dismissal(self) -> bool {
        self.open
    }
}

type ChangeHandler = Rc<dyn Fn(bool, PopoverChangeReason, &mut Window, &mut App) + 'static>;

/// A controlled, trigger/content anchored popover.
///
/// The caller must apply a requested value from [`Self::on_open_change`] to
/// the next render. This component never mutates its own open value.
#[derive(IntoElement)]
pub struct Popover {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    state: PopoverState,
    placement: PopoverPlacement,
    variant: PopoverVariant,
    trigger: AnyElement,
    content: AnyElement,
    on_change: Option<ChangeHandler>,
    debug_selector: Option<SharedString>,
    root_style: StyleRefinement,
}

impl Popover {
    /// Constructs a controlled popover from caller-owned trigger and content
    /// slots.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        open: bool,
        trigger: impl IntoElement,
        content: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            state: PopoverState::new(open, false),
            placement: PopoverPlacement::default(),
            variant: PopoverVariant::Default,
            trigger: trigger.into_any_element(),
            content: content.into_any_element(),
            on_change: None,
            debug_selector: None,
            root_style: StyleRefinement::default(),
        }
    }

    /// Sets whether the trigger refuses activation and tab traversal.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.state = PopoverState::new(self.state.open(), disabled);
        self
    }

    /// Selects the default card or caller-material bare variant.
    #[must_use]
    pub const fn variant(mut self, variant: PopoverVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Sets the preferred side.
    #[must_use]
    pub const fn side(mut self, side: PopoverSide) -> Self {
        self.placement = self.placement.side(side);
        self
    }

    /// Sets the alignment.
    #[must_use]
    pub const fn align(mut self, align: PopoverAlign) -> Self {
        self.placement = self.placement.align(align);
        self
    }

    /// Sets the main-axis separation.
    #[must_use]
    pub const fn side_offset(mut self, side_offset: Pixels) -> Self {
        self.placement = self.placement.side_offset(side_offset);
        self
    }

    /// Sets the cross-axis adjustment.
    #[must_use]
    pub const fn align_offset(mut self, align_offset: Pixels) -> Self {
        self.placement = self.placement.align_offset(align_offset);
        self
    }

    /// Sets both placement offsets.
    #[must_use]
    pub const fn offset(mut self, offset: PopoverOffset) -> Self {
        self.placement = self.placement.offset(offset);
        self
    }

    /// Replaces all placement preferences.
    #[must_use]
    pub const fn placement(mut self, placement: PopoverPlacement) -> Self {
        self.placement = placement;
        self
    }

    /// Emits a requested open value with its interaction reason.
    #[must_use]
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, PopoverChangeReason, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Alias for [`Self::on_open_change`].
    #[must_use]
    pub fn on_change(
        self,
        handler: impl Fn(bool, PopoverChangeReason, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change(handler)
    }

    /// Adds stable root, trigger, and content debug selectors.
    ///
    /// The trigger and content selectors append `-trigger` and `-content`.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Alias for [`Self::debug_selector`].
    #[must_use]
    pub fn debug_selector_prefix(self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector(selector)
    }

    /// Returns the controlled state captured by this render value.
    #[must_use]
    pub const fn state(&self) -> PopoverState {
        self.state
    }

    /// Returns the controlled open value captured by this render value.
    #[must_use]
    pub const fn open(&self) -> bool {
        self.state.open()
    }

    /// Returns whether the popover is controlled open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.state.is_open()
    }

    /// Returns whether trigger activation is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.state.is_disabled()
    }

    /// Returns the configured variant.
    #[must_use]
    pub const fn variant_value(&self) -> PopoverVariant {
        self.variant
    }

    /// Returns the configured placement.
    #[must_use]
    pub const fn placement_value(&self) -> PopoverPlacement {
        self.placement
    }

    /// Returns the next open value requested by trigger activation.
    #[must_use]
    pub const fn requested_toggle(&self) -> Option<bool> {
        self.state.requested_toggle()
    }

    /// Resolves the content recipe against the configured theme.
    #[must_use]
    pub fn resolved_style(&self) -> PopoverStyle {
        PopoverStyle::resolve(self.theme, self.variant)
    }
}

impl Styled for Popover {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.root_style
    }
}

impl RenderOnce for Popover {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            focus,
            theme,
            state,
            placement,
            variant,
            trigger,
            content,
            on_change,
            debug_selector,
            root_style,
        } = self;

        let trigger_bounds = Rc::new(RefCell::new(None::<Bounds<Pixels>>));
        let content_bounds = Rc::new(RefCell::new(None::<Bounds<Pixels>>));
        let root_selector = debug_selector.map(|selector| selector.to_string());
        let trigger_selector = root_selector
            .as_ref()
            .map(|selector| format!("{selector}-trigger"));
        let content_selector = root_selector
            .as_ref()
            .map(|selector| format!("{selector}-content"));

        let trigger_probe = bounds_probe(Rc::clone(&trigger_bounds));
        let focus = focus.tab_index(0).tab_stop(!state.disabled());
        let mut trigger_shell = div()
            .relative()
            .min_w_0()
            .child(trigger)
            .child(trigger_probe)
            .id(ElementId::NamedChild(
                Arc::new(id.clone()),
                SharedString::from("trigger"),
            ));

        if !state.disabled() {
            trigger_shell = trigger_shell.track_focus(&focus);
        }

        if let Some(selector) = trigger_selector {
            trigger_shell = trigger_shell.debug_selector(move || selector.clone());
        }

        if let Some(requested_open) = state.requested_toggle() {
            let handler = on_change.clone();
            trigger_shell = trigger_shell.on_click(move |event: &ClickEvent, window, cx| {
                if event.standard_click()
                    && let Some(handler) = handler.as_ref()
                {
                    handler(requested_open, PopoverChangeReason::Trigger, window, cx);
                }
            });
        }

        let escape_handler = on_change.clone();
        let open = state.open();
        let mut root = div()
            .relative()
            .min_w_0()
            .on_key_down(move |event, window, cx| {
                dismiss_on_escape(event, open, escape_handler.as_ref(), window, cx);
            })
            .child(trigger_shell);

        root.style().refine(&root_style);
        if let Some(selector) = root_selector {
            root = root.debug_selector(move || selector.clone());
        }

        let content = if open {
            let content_change = on_change.clone();
            let mut content_shell = popover_content(PopoverStyle::resolve(theme, variant), content)
                .absolute()
                .on_key_down(move |event, window, cx| {
                    dismiss_on_escape(event, true, content_change.as_ref(), window, cx);
                });

            if let Some(selector) = content_selector {
                content_shell = content_shell.debug_selector(move || selector.clone());
            }

            Some(deferred(content_shell).into_any_element())
        } else {
            None
        };

        let outside_handler = on_change;
        let content_bounds_for_outside = Rc::clone(&content_bounds);
        root = root.on_mouse_down_out(move |event, window, cx| {
            if !open {
                return;
            }
            let inside_content = content_bounds_for_outside
                .borrow()
                .as_ref()
                .is_some_and(|bounds| bounds.contains(&event.position));
            if !inside_content && let Some(handler) = outside_handler.as_ref() {
                handler(false, PopoverChangeReason::Outside, window, cx);
            }
        });

        PopoverElement::new(
            root.id(id),
            content,
            placement,
            trigger_bounds,
            content_bounds,
        )
    }
}

fn bounds_probe(destination: Rc<RefCell<Option<Bounds<Pixels>>>>) -> gpui::Canvas<()> {
    canvas(
        move |bounds, _, _| {
            *destination.borrow_mut() = Some(bounds);
        },
        |_, (), _, _| {},
    )
    .absolute()
    .top(px(0.0))
    .left(px(0.0))
    .size_full()
}

fn dismiss_on_escape(
    event: &KeyDownEvent,
    open: bool,
    handler: Option<&ChangeHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    if open && event.keystroke.key.as_str() == "escape" {
        window.prevent_default();
        cx.stop_propagation();
        if let Some(handler) = handler {
            handler(false, PopoverChangeReason::Escape, window, cx);
        }
    }
}

struct PopoverLayoutState {
    root_layout: LayoutId,
    content_layout: Option<LayoutId>,
}

struct PopoverElement {
    root: AnyElement,
    content: Option<AnyElement>,
    placement: PopoverPlacement,
    trigger_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    content_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl PopoverElement {
    fn new(
        root: impl IntoElement,
        content: Option<AnyElement>,
        placement: PopoverPlacement,
        trigger_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
        content_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    ) -> Self {
        Self {
            root: root.into_any_element(),
            content,
            placement,
            trigger_bounds,
            content_bounds,
        }
    }
}

impl IntoElement for PopoverElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PopoverElement {
    type RequestLayoutState = PopoverLayoutState;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let root_layout = self.root.request_layout(window, cx);
        let content_layout = self
            .content
            .as_mut()
            .map(|content| content.request_layout(window, cx));

        let mut child_layouts = Vec::with_capacity(2);
        child_layouts.push(root_layout);
        if let Some(content_layout) = content_layout {
            child_layouts.push(content_layout);
        }

        let style = Style {
            position: Position::Relative,
            display: Display::Flex,
            ..Style::default()
        };
        let layout = window.request_layout(style, child_layouts, cx);

        (
            layout,
            PopoverLayoutState {
                root_layout,
                content_layout,
            },
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.root.prepaint(window, cx);

        let Some(content_layout) = state.content_layout else {
            *self.content_bounds.borrow_mut() = None;
            return;
        };

        let root_bounds = window.layout_bounds(state.root_layout);
        let anchor = (*self.trigger_bounds.borrow()).unwrap_or(root_bounds);
        let content_size = window.layout_bounds(content_layout).size;
        let geometry = self
            .placement
            .resolve(anchor, content_size, window.viewport_size());

        *self.content_bounds.borrow_mut() = Some(geometry.bounds());

        let offset = geometry.origin - window.layout_bounds(content_layout).origin;
        let offset = point(offset.x.round(), offset.y.round());

        if let Some(content) = self.content.as_mut() {
            window.with_element_offset(offset, |window| {
                content.prepaint(window, cx);
            });
        }
    }

    fn paint(
        &mut self,
        _id: Option<&gpui::GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _state: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.root.paint(window, cx);
        if let Some(content) = self.content.as_mut() {
            content.paint(window, cx);
        }
    }
}
