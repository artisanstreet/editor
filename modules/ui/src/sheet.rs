//! Native controlled sheet primitive for GPUI.
//!
//! The sheet owns a full-viewport overlay and a side-docked panel. The caller
//! owns the open value: close handlers report a request to render the sheet
//! closed and never mutate the value captured by this render.
//!
//! The legacy frontend sheet (`modules/frontend/src/lib/components/ui/sheet`)
//! is a `bits-ui` dialog with four sides (`top`/`right`/`bottom`/`left`),
//! a `bg-black/80` overlay, popover-material panel, and a ghost close
//! affordance. This module transcribes those tokens into typed GPUI geometry,
//! style, focus, and event contracts.
//!
//! GPUI 0.2.2 does not provide a portal, platform modality flag, or
//! accessibility tree. The root fills the sized host supplied by its caller,
//! [`SheetFocusIntent`] exposes explicit focus-entry/restoration policy, and
//! overlay occlusion is limited to GPUI pointer hit testing.

use std::{rc::Rc, sync::Arc, time::Duration};

use artisan_assets::AssetId;
use gpui::prelude::Refineable as _;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, ColorExt as _, Div, ElementId, FocusHandle,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point,
    RenderOnce, SharedString, StyleRefinement, Styled, Window, div, point, px, size,
};

use crate::button::{AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant};
use crate::motion::MotionPolicy;
use crate::theme::ArtisanTheme;

/// Opacity of the legacy `bg-black/80` sheet backdrop.
pub const SHEET_OVERLAY_OPACITY: f32 = 0.8;

/// Maximum panel thickness for side-docked sheets, the legacy `sm:max-w-sm`
/// bound (24 rem at the 16 px root = 384 px).
pub const SHEET_DEFAULT_WIDTH: Pixels = px(384.0);

/// Fraction of the viewport edge consumed before the maximum-width cap.
///
/// Mirrors the frontend `w-3/4` utility for left/right sheets.
pub const SHEET_WIDTH_FRACTION: f32 = 0.75;

/// Default vertical thickness for top/bottom sheets before content sizing.
///
/// Matches [`SHEET_DEFAULT_WIDTH`] so top/bottom panels remain bounded without
/// content measurement.
pub const SHEET_DEFAULT_HEIGHT: Pixels = px(384.0);

/// One-pixel border width used on the panel's inset edge.
pub const SHEET_BORDER_WIDTH: Pixels = px(1.0);

/// Default close control accessible name retained from the frontend sheet.
pub const SHEET_CLOSE_LABEL: &str = "Close";

/// Semantic role retained for future platform accessibility wiring.
pub const SHEET_ROLE: &str = "dialog";

/// Open transition duration retained from the frontend `duration-200`.
pub const SHEET_OPEN_ANIMATION_DURATION: Duration = Duration::from_millis(200);

/// Explicit side placement for the sheet panel.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SheetSide {
    /// Panel is anchored to the top viewport edge.
    Top,
    /// Panel is anchored to the right viewport edge (frontend default).
    #[default]
    Right,
    /// Panel is anchored to the bottom viewport edge.
    Bottom,
    /// Panel is anchored to the left viewport edge.
    Left,
}

impl SheetSide {
    /// Returns the opposite edge.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Top => Self::Bottom,
            Self::Right => Self::Left,
            Self::Bottom => Self::Top,
            Self::Left => Self::Right,
        }
    }

    /// Returns whether this side docks on the horizontal axis.
    #[must_use]
    pub const fn is_horizontal(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// Returns whether this side docks on the vertical axis.
    #[must_use]
    pub const fn is_vertical(self) -> bool {
        matches!(self, Self::Top | Self::Bottom)
    }

    /// Returns the canonical string used for debug selectors and semantics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Top => "top",
            Self::Right => "right",
            Self::Bottom => "bottom",
            Self::Left => "left",
        }
    }
}

/// Why a caller-owned sheet was asked to close.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SheetDismissReason {
    /// The focused sheet received an unmodified Escape key event.
    Escape,
    /// The caller pressed the backdrop outside the panel surface.
    Overlay,
    /// The built-in ghost close button was activated.
    CloseButton,
}

/// Controlled open state captured by a [`Sheet`] render.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SheetState {
    open: bool,
}

impl SheetState {
    /// Creates controlled state from the caller's open value.
    #[must_use]
    pub const fn new(open: bool) -> Self {
        Self { open }
    }

    /// Returns the controlled open value.
    #[must_use]
    pub const fn open(self) -> bool {
        self.open
    }

    /// Returns the controlled open value.
    #[must_use]
    pub const fn is_open(self) -> bool {
        self.open
    }

    /// Returns the value the caller should apply after a dismissal request.
    ///
    /// A closed sheet cannot produce a new dismissal request.
    #[must_use]
    pub const fn requested_dismissal(self, _reason: SheetDismissReason) -> Option<bool> {
        if self.open { Some(false) } else { None }
    }

    /// Returns the value the caller should apply after a close request.
    #[must_use]
    pub const fn requested_close(self) -> Option<bool> {
        self.requested_dismissal(SheetDismissReason::CloseButton)
    }
}

/// Deterministic focus transition at a controlled open-state edge.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SheetFocusTransition {
    /// The controlled state did not cross an open/closed edge.
    Unchanged,
    /// The caller should focus the sheet's chosen entry handle.
    Enter,
    /// The caller should focus the handle captured before opening.
    Restore,
}

/// Resolves a controlled open-state edge without performing focus side effects.
#[must_use]
pub const fn sheet_focus_transition(previous_open: bool, next_open: bool) -> SheetFocusTransition {
    match (previous_open, next_open) {
        (false, true) => SheetFocusTransition::Enter,
        (true, false) => SheetFocusTransition::Restore,
        _ => SheetFocusTransition::Unchanged,
    }
}

/// Caller-invoked focus-entry and focus-restoration intent.
///
/// Call [`Self::apply`] once when the controlled value crosses an edge.
/// This helper does not install a focus trap or intercept Tab traversal.
pub struct SheetFocusIntent {
    entry: FocusHandle,
    restore: FocusHandle,
}

impl SheetFocusIntent {
    /// Creates an intent with the handle to focus on entry and the handle to
    /// restore after close.
    #[must_use]
    pub fn new(entry: FocusHandle, restore: FocusHandle) -> Self {
        Self { entry, restore }
    }

    /// Returns the handle selected for sheet entry.
    #[must_use]
    pub fn entry(&self) -> &FocusHandle {
        &self.entry
    }

    /// Returns the handle selected for restoration after close.
    #[must_use]
    pub fn restore(&self) -> &FocusHandle {
        &self.restore
    }

    /// Applies the edge transition and returns the action taken.
    #[must_use]
    pub fn apply(
        &self,
        previous_open: bool,
        next_open: bool,
        window: &mut Window,
        cx: &mut App,
    ) -> SheetFocusTransition {
        let transition = sheet_focus_transition(previous_open, next_open);
        match transition {
            SheetFocusTransition::Unchanged => {}
            SheetFocusTransition::Enter => self.entry.focus(window, cx),
            SheetFocusTransition::Restore => self.restore.focus(window, cx),
        }
        transition
    }
}

/// Resolved panel and overlay rectangles for one viewport and side.
///
/// The overlay always fills the viewport; the panel is inset to the selected
/// edge and bounded by the width fraction and maximum-width cap for left/right
/// sides, or the default height cap for top/bottom sides. All geometry is
/// deterministic and does not require a GPUI layout pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetGeometry {
    /// Viewport used to resolve the bounds.
    pub viewport: gpui::Size<Pixels>,
    /// Resolved panel rectangle.
    pub panel: Bounds<Pixels>,
    /// Resolved overlay rectangle (always the viewport inset).
    pub overlay: Bounds<Pixels>,
    /// Side used to place the panel.
    pub side: SheetSide,
    /// Resolved panel thickness on the side axis.
    pub thickness: Pixels,
}

impl SheetGeometry {
    /// Resolves panel and overlay bounds from a viewport, side, and desired
    /// thickness before capping.
    ///
    /// Horizontal sides (`left`/`right`) treat thickness as width; vertical
    /// sides (`top`/`bottom`) treat thickness as height. Thickness is bounded
    /// by zero, the viewport extent on that axis, the legacy `w-3/4`
    /// equivalent, and the `sm:max-w-sm` / default-height cap.
    #[must_use]
    pub fn resolve(
        viewport: gpui::Size<Pixels>,
        side: SheetSide,
        desired_thickness: Pixels,
        max_width: Pixels,
        width_fraction: f32,
    ) -> Self {
        let viewport_bounds = Bounds::new(point(px(0.0), px(0.0)), viewport);
        let fraction = width_fraction.clamp(0.0, 1.0);
        let desired = f32::from(desired_thickness).max(0.0);

        let thickness = if side.is_horizontal() {
            let cap = f32::from(max_width).max(0.0);
            let fractional = f32::from(viewport.width) * fraction;
            px(desired
                .min(cap)
                .min(fractional)
                .min(f32::from(viewport.width)))
        } else {
            let cap = f32::from(max_width).max(0.0);
            let fractional = f32::from(viewport.height) * fraction;
            // Top/bottom use the same cap/fraction policy as left/right for
            // symmetry; callers needing content-height should pass that height
            // as `desired_thickness` before calling this function.
            px(desired
                .min(cap)
                .min(fractional)
                .min(f32::from(viewport.height)))
        };

        let panel = match side {
            SheetSide::Right => Bounds::new(
                point(viewport.width - thickness, px(0.0)),
                size(thickness, viewport.height),
            ),
            SheetSide::Left => {
                Bounds::new(point(px(0.0), px(0.0)), size(thickness, viewport.height))
            }
            SheetSide::Top => Bounds::new(point(px(0.0), px(0.0)), size(viewport.width, thickness)),
            SheetSide::Bottom => Bounds::new(
                point(px(0.0), viewport.height - thickness),
                size(viewport.width, thickness),
            ),
        };

        Self {
            viewport,
            panel,
            overlay: viewport_bounds,
            side,
            thickness,
        }
    }

    /// Convenience that resolves with the legacy sheet tokens.
    #[must_use]
    pub fn with_defaults(viewport: gpui::Size<Pixels>, side: SheetSide) -> Self {
        let desired = if side.is_horizontal() {
            SHEET_DEFAULT_WIDTH
        } else {
            SHEET_DEFAULT_HEIGHT
        };
        Self::resolve(
            viewport,
            side,
            desired,
            SHEET_DEFAULT_WIDTH,
            SHEET_WIDTH_FRACTION,
        )
    }

    /// Returns whether a point lies inside the panel rectangle.
    #[must_use]
    pub fn panel_contains(&self, point: Point<Pixels>) -> bool {
        point.x >= self.panel.origin.x
            && point.y >= self.panel.origin.y
            && point.x < self.panel.origin.x + self.panel.size.width
            && point.y < self.panel.origin.y + self.panel.size.height
    }

    /// Returns whether a point lies inside the overlay rectangle.
    #[must_use]
    pub fn overlay_contains(&self, point: Point<Pixels>) -> bool {
        point.x >= self.overlay.origin.x
            && point.y >= self.overlay.origin.y
            && point.x < self.overlay.origin.x + self.overlay.size.width
            && point.y < self.overlay.origin.y + self.overlay.size.height
    }
}

/// Single 200 ms open animation retained by the sheet recipe.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SheetAnimation;

impl SheetAnimation {
    /// Returns the open transition duration.
    #[must_use]
    pub const fn duration(self) -> Duration {
        SHEET_OPEN_ANIMATION_DURATION
    }

    /// Creates GPUI's safe linear clock for this animation.
    #[must_use]
    pub fn gpui_clock(self) -> Animation {
        Animation::new(self.duration())
    }
}

/// Motion decision for the sheet's opening presentation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SheetMotionPlan {
    /// Present the final state without an animation.
    Immediate,
    /// Fade and slide the mounted sheet from transparent/off-edge to opaque.
    Animate(SheetAnimation),
}

impl SheetMotionPlan {
    /// Resolves the sheet transition under the shared motion policy.
    #[must_use]
    pub const fn for_policy(policy: MotionPolicy) -> Self {
        match policy {
            MotionPolicy::Full => Self::Animate(SheetAnimation),
            MotionPolicy::Reduced => Self::Immediate,
        }
    }

    /// Returns the animation only for the animated plan.
    #[must_use]
    pub const fn animation(self) -> Option<SheetAnimation> {
        match self {
            Self::Immediate => None,
            Self::Animate(animation) => Some(animation),
        }
    }
}

/// Resolved theme, geometry, control, and motion values for a sheet.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SheetStyle {
    /// Black backdrop at [`SHEET_OVERLAY_OPACITY`].
    pub overlay: Hsla,
    /// Exact backdrop opacity retained for deterministic assertions.
    pub overlay_opacity: f32,
    /// Popover surface used by the panel.
    pub background: Hsla,
    /// Popover foreground used by the panel.
    pub foreground: Hsla,
    /// Muted foreground used by the optional description.
    pub description_foreground: Hsla,
    /// One-pixel border color on the panel's inset edge.
    pub border: Hsla,
    /// One-pixel border width.
    pub border_width: Pixels,
    /// Legacy `p-6` panel padding.
    pub padding: Pixels,
    /// Header title/description gap (`gap-1.5`).
    pub header_gap: Pixels,
    /// Gap between header and caller content.
    pub content_gap: Pixels,
    /// Close button `top-4 right-4` inset.
    pub close_inset: Pixels,
    /// Maximum panel thickness before the viewport-fraction cap.
    pub max_width: Pixels,
    /// Viewport fraction applied before the max-width cap.
    pub width_fraction: f32,
    /// Dialog-title typography used for the sheet title.
    pub title_size: Pixels,
    /// Sheet title weight.
    pub title_weight: FontWeight,
    /// Description text size.
    pub description_size: Pixels,
    /// Shared ghost icon-small recipe used by the close affordance.
    pub close_button: crate::button::ButtonStyle,
    /// Selected opening motion plan.
    pub motion: SheetMotionPlan,
}

impl SheetStyle {
    /// Resolves the audited sheet recipe from the shared theme and motion
    /// policy.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, motion: MotionPolicy) -> Self {
        let padding = theme.spacing.steps(6.0);
        Self {
            overlay: gpui::black().opacity(SHEET_OVERLAY_OPACITY),
            overlay_opacity: SHEET_OVERLAY_OPACITY,
            background: theme.colors.popover.to_paint(),
            foreground: theme.colors.popover_foreground.to_paint(),
            description_foreground: theme.colors.muted_foreground.to_paint(),
            border: theme.colors.border.to_paint(),
            border_width: SHEET_BORDER_WIDTH,
            padding,
            header_gap: theme.spacing.steps(1.5),
            content_gap: theme.spacing.steps(4.0),
            close_inset: theme.spacing.steps(4.0),
            max_width: SHEET_DEFAULT_WIDTH,
            width_fraction: SHEET_WIDTH_FRACTION,
            title_size: theme.typography.dialog_title_text,
            title_weight: FontWeight::from(f32::from(theme.typography.dialog_title_weight)),
            description_size: theme.typography.control_text,
            close_button: crate::button::ButtonStyle::resolve(
                theme,
                ButtonVariant::Ghost,
                ButtonSize::IconSmall,
                motion,
            ),
            motion: SheetMotionPlan::for_policy(motion),
        }
    }
}

/// Semantic metadata retained by the sheet for callers and future platform
/// accessibility adapters.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SheetSemantics {
    /// Retained semantic role.
    pub role: &'static str,
    /// Visible title string.
    pub title: SharedString,
    /// Optional visible description string.
    pub description: Option<SharedString>,
    /// Retained accessible name for the icon-only close control.
    pub close_label: SharedString,
    /// Selected side placement.
    pub side: SheetSide,
}

type DismissHandler = Rc<dyn Fn(SheetDismissReason, &mut Window, &mut App) + 'static>;

/// Controlled native sheet with a caller-owned content slot.
#[derive(IntoElement)]
pub struct Sheet {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    motion: MotionPolicy,
    state: SheetState,
    side: SheetSide,
    title: SharedString,
    description: Option<SharedString>,
    content: AnyElement,
    close_label: AccessibleLabel,
    on_dismiss: Option<DismissHandler>,
    debug_selector: Option<SharedString>,
    root_style: StyleRefinement,
}

impl Sheet {
    /// Constructs a sheet from a caller-owned open value, side, and content
    /// slot.
    ///
    /// Dismissal remains controlled: a handler installed with
    /// [`Self::on_dismiss`] receives a reason and should rerender this sheet
    /// with `open = false`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        open: bool,
        side: SheetSide,
        title: impl Into<SharedString>,
        content: impl IntoElement,
    ) -> Self {
        Self::with_state(
            id,
            focus,
            theme,
            SheetState::new(open),
            side,
            title,
            content,
        )
    }

    /// Constructs a sheet from an already typed controlled state.
    ///
    /// # Panics
    ///
    /// Panics only if the built-in non-empty close label is rejected.
    #[must_use]
    pub fn with_state(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        state: SheetState,
        side: SheetSide,
        title: impl Into<SharedString>,
        content: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            motion: MotionPolicy::Full,
            state,
            side,
            title: title.into(),
            description: None,
            content: content.into_any_element(),
            close_label: AccessibleLabel::new(SHEET_CLOSE_LABEL)
                .expect("the built-in sheet close label is non-empty"),
            on_dismiss: None,
            debug_selector: None,
            root_style: StyleRefinement::default(),
        }
    }

    /// Replaces the controlled state captured by this render value.
    #[must_use]
    pub const fn controlled_state(mut self, state: SheetState) -> Self {
        self.state = state;
        self
    }

    /// Selects the motion policy for this render value.
    #[must_use]
    pub const fn motion_policy(mut self, motion: MotionPolicy) -> Self {
        self.motion = motion;
        self
    }

    /// Replaces the side placement.
    #[must_use]
    pub const fn side(mut self, side: SheetSide) -> Self {
        self.side = side;
        self
    }

    /// Adds the optional visible description and its retained semantic value.
    #[must_use]
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Installs the caller-owned dismissal callback.
    #[must_use]
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(SheetDismissReason, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// Adds a stable selector prefix for the root and inspectable sheet parts.
    ///
    /// The root receives the supplied selector; overlay, panel, title,
    /// description, and close controls receive `-overlay`, `-panel`,
    /// `-title`, `-description`, and `-close` suffixes.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Alias that makes the selector-prefix convention explicit at call sites.
    #[must_use]
    pub fn debug_selector_prefix(self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector(selector)
    }

    /// Returns the controlled state captured by this render value.
    #[must_use]
    pub const fn state(&self) -> SheetState {
        self.state
    }

    /// Returns the controlled open value captured by this render value.
    #[must_use]
    pub const fn open(&self) -> bool {
        self.state.open()
    }

    /// Returns the controlled open value captured by this render value.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.state.is_open()
    }

    /// Returns the selected side placement.
    #[must_use]
    pub const fn side_value(&self) -> SheetSide {
        self.side
    }

    /// Returns the requested controlled value for a dismissal reason.
    #[must_use]
    pub const fn requested_dismissal(&self, reason: SheetDismissReason) -> Option<bool> {
        self.state.requested_dismissal(reason)
    }

    /// Returns the resolved theme/motion recipe.
    #[must_use]
    pub fn visual_style(&self) -> SheetStyle {
        SheetStyle::resolve(self.theme, self.motion)
    }

    /// Returns the retained title/description/close semantic metadata.
    #[must_use]
    pub fn semantics(&self) -> SheetSemantics {
        SheetSemantics {
            role: SHEET_ROLE,
            title: self.title.clone(),
            description: self.description.clone(),
            close_label: SharedString::from(self.close_label.as_str().to_owned()),
            side: self.side,
        }
    }

    /// Returns the focus handle tracked by the built-in close control.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Resolves the panel geometry for the current viewport.
    #[must_use]
    pub fn geometry(&self, viewport: gpui::Size<Pixels>) -> SheetGeometry {
        let style = self.visual_style();
        let desired = if self.side.is_horizontal() {
            style.max_width
        } else {
            SHEET_DEFAULT_HEIGHT
        };
        SheetGeometry::resolve(
            viewport,
            self.side,
            desired,
            style.max_width,
            style.width_fraction,
        )
    }
}

impl Styled for Sheet {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.root_style
    }
}

/// Renders the zero-sized sentinel painted for a closed sheet.
fn render_closed_sheet(id: ElementId, debug_selector: Option<SharedString>) -> AnyElement {
    let mut closed = div()
        .id(ElementId::NamedChild(Arc::new(id), "closed".into()))
        .absolute()
        .w(px(0.0))
        .h(px(0.0));
    if let Some(selector) = debug_selector {
        let selector = format!("{selector}-closed");
        closed = closed.debug_selector(move || selector);
    }
    closed.into_any_element()
}

/// Renders the title header with its optional description row.
fn render_sheet_header(
    title: &SharedString,
    description: Option<&SharedString>,
    style: &SheetStyle,
    title_selector: Option<String>,
    description_selector: Option<String>,
) -> Div {
    let mut title_element = div()
        .min_w_0()
        .text_size(style.title_size)
        .font_weight(style.title_weight)
        .text_color(style.foreground)
        .child(title.clone());
    if let Some(selector) = title_selector {
        title_element = title_element.debug_selector(move || selector);
    }

    let mut header = div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(style.header_gap)
        .pr(style.close_button.width.unwrap_or(px(0.0)) + style.close_inset)
        .child(title_element);

    if let Some(description) = description {
        let mut description_element = div()
            .min_w_0()
            .text_size(style.description_size)
            .text_color(style.description_foreground)
            .child(description.clone());
        if let Some(selector) = description_selector {
            description_element = description_element.debug_selector(move || selector);
        }
        header = header.child(description_element);
    }
    header
}

/// Renders the ghost close control with its absolute wrapper.
fn render_sheet_close_button(
    id: &ElementId,
    focus: &FocusHandle,
    theme: &ArtisanTheme,
    motion: MotionPolicy,
    close_label: &AccessibleLabel,
    on_dismiss: Option<&DismissHandler>,
    close_selector: Option<String>,
) -> Div {
    let close_dismiss = on_dismiss.cloned();
    let mut close_button = Button::new(
        (id.clone(), "close"),
        focus.clone(),
        *theme,
        motion,
        ButtonVariant::Ghost,
        ButtonSize::IconSmall,
        ButtonContent::icon_only(AssetId::TABLER_X, close_label.clone()),
    )
    .expect("the built-in icon-only close button has a valid label and size")
    .on_activate(move |event, window, cx| {
        if event.standard_click()
            && let Some(handler) = close_dismiss.as_ref()
        {
            handler(SheetDismissReason::CloseButton, window, cx);
        }
    });

    if let Some(selector) = close_selector {
        close_button = close_button.debug_selector(selector);
    }

    div()
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .child(close_button)
}

/// Renders the backdrop overlay with its dismiss handler.
fn render_sheet_overlay(
    style: &SheetStyle,
    on_dismiss: Option<&DismissHandler>,
    overlay_selector: Option<String>,
) -> Div {
    let overlay_dismiss = on_dismiss.cloned();
    let mut overlay = div()
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .left(px(0.0))
        .bg(style.overlay)
        .occlude()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            if let Some(handler) = overlay_dismiss.as_ref() {
                handler(SheetDismissReason::Overlay, window, cx);
            }
            cx.stop_propagation();
        });

    if let Some(selector) = overlay_selector {
        overlay = overlay.debug_selector(move || selector);
    }
    overlay
}

/// Renders the docked panel with header, content, and close control.
fn render_sheet_panel(
    style: &SheetStyle,
    geometry: &SheetGeometry,
    side: SheetSide,
    panel_selector: Option<String>,
    header: Div,
    content: AnyElement,
    close: Div,
) -> Div {
    let content_shell = div().min_w_0().flex_1().child(content);

    let mut panel = div()
        .absolute()
        .flex()
        .flex_col()
        .min_w_0()
        .gap(style.content_gap)
        .p(style.padding)
        .bg(style.background)
        .text_color(style.foreground)
        .occlude()
        .overflow_hidden()
        .child(header)
        .child(content_shell)
        .child(close);

    panel = match side {
        SheetSide::Right => panel
            .right(px(0.0))
            .top(px(0.0))
            .bottom(px(0.0))
            .w(geometry.thickness)
            .border_l_1()
            .border_color(style.border),
        SheetSide::Left => panel
            .left(px(0.0))
            .top(px(0.0))
            .bottom(px(0.0))
            .w(geometry.thickness)
            .border_r_1()
            .border_color(style.border),
        SheetSide::Top => panel
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .h(geometry.thickness)
            .border_b_1()
            .border_color(style.border),
        SheetSide::Bottom => panel
            .bottom(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .h(geometry.thickness)
            .border_t_1()
            .border_color(style.border),
    };

    if let Some(selector) = panel_selector {
        panel = panel.debug_selector(move || selector);
    }
    panel
}

/// Renders the fullscreen root with overlay, panel, and escape dismissal.
fn render_sheet_root(
    overlay: Div,
    panel: Div,
    on_dismiss: Option<&DismissHandler>,
    root_style: &StyleRefinement,
    selector: Option<String>,
) -> Div {
    let escape_dismiss = on_dismiss.cloned();
    let mut root = div()
        .absolute()
        .top(px(0.0))
        .right(px(0.0))
        .bottom(px(0.0))
        .left(px(0.0))
        .flex()
        .child(overlay)
        .child(panel)
        .on_key_down(move |event, window, cx| {
            if event.keystroke.key.eq_ignore_ascii_case("escape")
                && !event.keystroke.modifiers.modified()
            {
                window.prevent_default();
                if let Some(handler) = escape_dismiss.as_ref() {
                    handler(SheetDismissReason::Escape, window, cx);
                }
                cx.stop_propagation();
            }
        });

    root.style().refine(root_style);
    if let Some(selector) = selector {
        root = root.debug_selector(move || selector);
    }
    root
}

/// Applies the open motion clock to the composed root.
fn animate_sheet_root(root: Div, motion: SheetMotionPlan, id: ElementId) -> AnyElement {
    match motion {
        SheetMotionPlan::Immediate => root.into_any_element(),
        SheetMotionPlan::Animate(animation) => root
            .with_animation((id, "open"), animation.gpui_clock(), |element, progress| {
                element.opacity(progress)
            })
            .into_any_element(),
    }
}

impl RenderOnce for Sheet {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if !self.state.is_open() {
            return render_closed_sheet(self.id, self.debug_selector);
        }

        let Self {
            id,
            focus,
            theme,
            motion,
            side,
            title,
            description,
            content,
            close_label,
            on_dismiss,
            debug_selector,
            root_style,
            ..
        } = self;

        let style = SheetStyle::resolve(theme, motion);
        let viewport = window.viewport_size();
        let desired = if side.is_horizontal() {
            style.max_width
        } else {
            SHEET_DEFAULT_HEIGHT
        };
        let geometry = SheetGeometry::resolve(
            viewport,
            side,
            desired,
            style.max_width,
            style.width_fraction,
        );

        let selector = debug_selector.map(|selector| selector.to_string());
        let title_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-title"));
        let description_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-description"));
        let close_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-close"));
        let overlay_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-overlay"));
        let panel_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-panel"));

        let header = render_sheet_header(
            &title,
            description.as_ref(),
            &style,
            title_selector,
            description_selector,
        );
        let close = render_sheet_close_button(
            &id,
            &focus,
            &theme,
            motion,
            &close_label,
            on_dismiss.as_ref(),
            close_selector,
        );
        let overlay = render_sheet_overlay(&style, on_dismiss.as_ref(), overlay_selector);
        let panel = render_sheet_panel(
            &style,
            &geometry,
            side,
            panel_selector,
            header,
            content,
            close,
        );

        let shadows = theme.elevation.card_shadow;
        let panel_shadow = shadows
            .iter()
            .copied()
            .map(crate::theme::ShadowLayer::to_box_shadow)
            .collect::<Vec<_>>();
        let panel = panel.shadow(panel_shadow);

        let root = render_sheet_root(overlay, panel, on_dismiss.as_ref(), &root_style, selector);
        animate_sheet_root(root, style.motion, id)
    }
}
