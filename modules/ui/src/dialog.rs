//! Native controlled modal dialog primitive for GPUI.
//!
//! The dialog owns the visual overlay, centered content surface, title and
//! description metadata, close affordance, and dismissal event paths. The
//! caller owns the open value: dismissal handlers report a request to render
//! the dialog closed and never mutate the value captured by this render.
//!
//! GPUI 0.2.2 does not provide a portal, platform modality flag, accessibility
//! tree, or focus-trap primitive. The root therefore fills the positioned,
//! sized host supplied by its caller, [`DialogFocusIntent`] exposes explicit
//! focus-entry/restoration policy, and the overlay's occlusion is limited to
//! GPUI pointer hit testing. None of those mechanisms claims native window
//! modality or automatic focus trapping.

use std::{rc::Rc, time::Duration};

use artisan_assets::AssetId;
use gpui::prelude::Refineable;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, BoxShadow, ElementId, FocusHandle,
    FontWeight, Hsla, InteractiveElement, IntoElement, MouseButton, ParentElement, Pixels, Point,
    RenderOnce, SharedString, Size, StyleRefinement, Styled, Window, div, point, px, size,
};

use crate::button::{
    AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonStyle, ButtonVariant,
};
use crate::motion::MotionPolicy;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

/// The opacity of the legacy `bg-black/80` modal backdrop.
pub const BACKDROP_OPACITY: f32 = 0.8;

/// The default `sm:max-w-md` dialog content bound: 28 rem at the 16 px root.
pub const DEFAULT_MAX_WIDTH: Pixels = px(448.0);

/// The horizontal and vertical viewport inset retained from
/// `max-w-[calc(100%-2rem)]`.
pub const DEFAULT_VIEWPORT_MARGIN: Pixels = px(16.0);

/// The default close control's retained accessible name.
pub const DEFAULT_CLOSE_LABEL: &str = "Close";

/// Semantic role metadata retained for future platform accessibility wiring.
pub const DIALOG_ROLE: &str = "dialog";

/// The legacy dialog open transition duration.
pub const OPEN_ANIMATION_DURATION: Duration = Duration::from_millis(100);

/// Why a caller-owned dialog was asked to close.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DialogDismissReason {
    /// The focused dialog received an unmodified Escape key event.
    Escape,
    /// The caller pressed the modal backdrop outside the content surface.
    Backdrop,
    /// The built-in ghost close button was activated.
    CloseButton,
}

/// The controlled open state captured by a [`Dialog`] render.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct DialogState {
    open: bool,
}

impl DialogState {
    /// Creates a controlled state from the caller's open value.
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
    /// A closed dialog cannot produce a new dismissal request. The reason is
    /// accepted so callers can keep one typed state transition for Escape,
    /// backdrop, and close-button paths.
    #[must_use]
    pub const fn requested_dismissal(self, _reason: DialogDismissReason) -> Option<bool> {
        if self.open { Some(false) } else { None }
    }

    /// Returns the value the caller should apply after a close request.
    #[must_use]
    pub const fn requested_close(self) -> Option<bool> {
        self.requested_dismissal(DialogDismissReason::CloseButton)
    }
}

/// The deterministic focus transition at a controlled open-state edge.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DialogFocusTransition {
    /// The controlled state did not cross an open/closed edge.
    Unchanged,
    /// The caller should focus the dialog's chosen entry handle.
    Enter,
    /// The caller should focus the handle captured before opening.
    Restore,
}

/// Resolves a controlled open-state edge without performing focus side effects.
#[must_use]
pub const fn focus_transition(previous_open: bool, next_open: bool) -> DialogFocusTransition {
    match (previous_open, next_open) {
        (false, true) => DialogFocusTransition::Enter,
        (true, false) => DialogFocusTransition::Restore,
        _ => DialogFocusTransition::Unchanged,
    }
}

/// Caller-invoked focus-entry and focus-restoration intent.
///
/// Call [`Self::apply`] once when the controlled value crosses an edge. This
/// helper deliberately does not install a focus trap, intercept Tab traversal,
/// or run automatically from [`Dialog`]. Those policies are not primitives in
/// pinned GPUI 0.2.2 and remain with the caller/application focus coordinator.
pub struct DialogFocusIntent {
    entry: FocusHandle,
    restore: FocusHandle,
}

impl DialogFocusIntent {
    /// Creates an intent with the handle to focus on entry and the handle to
    /// restore after close.
    #[must_use]
    pub fn new(entry: FocusHandle, restore: FocusHandle) -> Self {
        Self { entry, restore }
    }

    /// Returns the handle selected for dialog entry.
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
    ///
    /// Repeated renders with the same state return
    /// [`DialogFocusTransition::Unchanged`] and do not move focus again.
    #[must_use]
    pub fn apply(
        &self,
        previous_open: bool,
        next_open: bool,
        window: &mut Window,
    ) -> DialogFocusTransition {
        let transition = focus_transition(previous_open, next_open);
        match transition {
            DialogFocusTransition::Unchanged => {}
            DialogFocusTransition::Enter => self.entry.focus(window),
            DialogFocusTransition::Restore => self.restore.focus(window),
        }
        transition
    }
}

/// Centered content bounds after applying a maximum width/height and a
/// non-negative viewport margin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DialogGeometry {
    /// The viewport used to resolve the bounds.
    pub viewport: Size<Pixels>,
    /// The resolved centered content rectangle.
    pub content: Bounds<Pixels>,
    /// The non-negative margin applied on every viewport edge.
    pub margin: Pixels,
}

impl DialogGeometry {
    /// Resolves a centered rectangle from desired content dimensions.
    ///
    /// Width is capped by `max_width`, height is capped by `max_height` when
    /// supplied, and both dimensions are capped by the viewport after margins.
    /// Negative desired dimensions and margins resolve to zero rather than
    /// producing an inverted rectangle.
    #[must_use]
    pub fn centered(
        viewport: Size<Pixels>,
        desired_content: Size<Pixels>,
        max_width: Pixels,
        max_height: Option<Pixels>,
        margin: Pixels,
    ) -> Self {
        let margin = px(f32::from(margin).max(0.0));
        let available = size(
            available_dimension(viewport.width, margin),
            available_dimension(viewport.height, margin),
        );
        let content_size = size(
            bounded_dimension(desired_content.width, Some(max_width), available.width),
            bounded_dimension(desired_content.height, max_height, available.height),
        );
        let origin = point(
            px((f32::from(viewport.width) - f32::from(content_size.width)) / 2.0),
            px((f32::from(viewport.height) - f32::from(content_size.height)) / 2.0),
        );

        Self {
            viewport,
            content: Bounds {
                origin,
                size: content_size,
            },
            margin,
        }
    }

    /// Returns whether a point lies inside the content rectangle.
    #[must_use]
    pub fn contains(&self, point: Point<Pixels>) -> bool {
        point.x >= self.content.origin.x
            && point.y >= self.content.origin.y
            && point.x < self.content.origin.x + self.content.size.width
            && point.y < self.content.origin.y + self.content.size.height
    }
}

fn available_dimension(viewport: Pixels, margin: Pixels) -> Pixels {
    px((f32::from(viewport) - 2.0 * f32::from(margin)).max(0.0))
}

fn bounded_dimension(desired: Pixels, maximum: Option<Pixels>, available: Pixels) -> Pixels {
    let mut bound = f32::from(available).max(0.0);
    if let Some(maximum) = maximum {
        bound = bound.min(f32::from(maximum).max(0.0));
    }
    px(f32::from(desired).max(0.0).min(bound))
}

/// The single 100 ms open animation retained by the dialog recipe.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DialogAnimation;

impl DialogAnimation {
    /// Returns the open transition duration.
    #[must_use]
    pub const fn duration(self) -> Duration {
        OPEN_ANIMATION_DURATION
    }

    /// Creates GPUI's safe linear clock for this animation.
    #[must_use]
    pub fn gpui_clock(self) -> Animation {
        Animation::new(self.duration())
    }
}

/// Motion decision for the dialog's opening presentation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DialogMotionPlan {
    /// Present the final state without an animation.
    Immediate,
    /// Fade the mounted dialog from transparent to opaque.
    Animate(DialogAnimation),
}

impl DialogMotionPlan {
    /// Resolves the dialog transition under the shared motion policy.
    #[must_use]
    pub const fn for_policy(policy: MotionPolicy) -> Self {
        match policy {
            MotionPolicy::Full => Self::Animate(DialogAnimation),
            MotionPolicy::Reduced => Self::Immediate,
        }
    }

    /// Returns the animation only for the animated plan.
    #[must_use]
    pub const fn animation(self) -> Option<DialogAnimation> {
        match self {
            Self::Immediate => None,
            Self::Animate(animation) => Some(animation),
        }
    }
}

/// Resolved theme, geometry, control, and motion values for a dialog.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DialogStyle {
    /// The black backdrop at [`BACKDROP_OPACITY`].
    pub overlay: Hsla,
    /// The exact backdrop opacity retained for deterministic assertions.
    pub overlay_opacity: f32,
    /// The popover surface used by dialog content.
    pub background: Hsla,
    /// The popover foreground used by dialog content.
    pub foreground: Hsla,
    /// The muted foreground used by the optional description.
    pub description_foreground: Hsla,
    /// The one-pixel foreground hairline color.
    pub ring_color: Hsla,
    /// The one-pixel foreground hairline spread.
    pub ring_spread: Pixels,
    /// The legacy `rounded-4xl` corner radius.
    pub corner_radius: Pixels,
    /// The legacy `p-6` content padding.
    pub padding: Pixels,
    /// The header title/description gap.
    pub header_gap: Pixels,
    /// The legacy `gap-6` gap between header and caller content.
    pub content_gap: Pixels,
    /// The close button's `top-4 right-4` inset.
    pub close_inset: Pixels,
    /// The default `sm:max-w-md` content width cap.
    pub max_width: Pixels,
    /// The viewport margin used to keep content inside small hosts.
    pub viewport_margin: Pixels,
    /// Dialog title text size from the shared typography tokens.
    pub title_size: Pixels,
    /// Dialog title weight from the shared typography tokens.
    pub title_weight: FontWeight,
    /// Optional description text size.
    pub description_size: Pixels,
    /// Shared ghost icon-small recipe used by the close affordance.
    pub close_button: ButtonStyle,
    /// The selected opening motion plan.
    pub motion: DialogMotionPlan,
}

impl DialogStyle {
    /// Resolves the audited dialog recipe from the shared theme and motion
    /// policy.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, motion: MotionPolicy) -> Self {
        let padding = theme.spacing.steps(6.0);
        Self {
            overlay: Hsla::black().opacity(BACKDROP_OPACITY),
            overlay_opacity: BACKDROP_OPACITY,
            background: theme.colors.popover.to_paint(),
            foreground: theme.colors.popover_foreground.to_paint(),
            description_foreground: theme.colors.muted_foreground.to_paint(),
            ring_color: theme.colors.foreground.with_alpha(0.10).to_paint(),
            ring_spread: px(1.0),
            corner_radius: RadiusTokens::value(RadiusStep::X4l),
            padding,
            header_gap: theme.spacing.steps(2.0),
            content_gap: padding,
            close_inset: theme.spacing.steps(4.0),
            max_width: DEFAULT_MAX_WIDTH,
            viewport_margin: DEFAULT_VIEWPORT_MARGIN,
            title_size: theme.typography.dialog_title_text,
            title_weight: FontWeight::from(f32::from(theme.typography.dialog_title_weight)),
            description_size: theme.typography.control_text,
            close_button: ButtonStyle::resolve(
                theme,
                ButtonVariant::Ghost,
                ButtonSize::IconSmall,
                motion,
            ),
            motion: DialogMotionPlan::for_policy(motion),
        }
    }

    /// Builds the zero-blur spread shadow used for the legacy ring hairline.
    #[must_use]
    pub fn ring(self) -> BoxShadow {
        BoxShadow {
            color: self.ring_color,
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: self.ring_spread,
        }
    }
}

/// Semantic metadata retained by the dialog for callers and future platform
/// accessibility adapters.
///
/// Pinned GPUI has no native accessibility tree, so these values are metadata;
/// the current renderer does not claim that a platform screen reader consumes
/// them.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct DialogSemantics {
    /// The retained semantic role.
    pub role: &'static str,
    /// The visible title string.
    pub title: SharedString,
    /// The optional visible description string.
    pub description: Option<SharedString>,
    /// The retained accessible name for the icon-only close control.
    pub close_label: SharedString,
}

type DismissHandler = Rc<dyn Fn(DialogDismissReason, &mut Window, &mut App) + 'static>;

/// A controlled native modal dialog with a caller-owned content slot.
#[derive(IntoElement)]
pub struct Dialog {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    motion: MotionPolicy,
    state: DialogState,
    title: SharedString,
    description: Option<SharedString>,
    content: AnyElement,
    close_label: AccessibleLabel,
    on_dismiss: Option<DismissHandler>,
    debug_selector: Option<SharedString>,
    root_style: StyleRefinement,
}

impl Dialog {
    /// Constructs a dialog from a caller-owned open value and content slot.
    ///
    /// Dismissal remains controlled: a handler installed with
    /// [`Self::on_dismiss`] receives a reason and should rerender this dialog
    /// with `open = false`.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        motion: MotionPolicy,
        open: bool,
        title: impl Into<SharedString>,
        content: impl IntoElement,
    ) -> Self {
        Self::with_state(
            id,
            focus,
            theme,
            motion,
            DialogState::new(open),
            title,
            content,
        )
    }

    /// Constructs a dialog from an already typed controlled state.
    ///
    /// # Panics
    ///
    /// Panics only if the built-in non-empty close label is rejected.
    #[must_use]
    pub fn with_state(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        motion: MotionPolicy,
        state: DialogState,
        title: impl Into<SharedString>,
        content: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            motion,
            state,
            title: title.into(),
            description: None,
            content: content.into_any_element(),
            close_label: AccessibleLabel::new(DEFAULT_CLOSE_LABEL)
                .expect("the built-in dialog close label is non-empty"),
            on_dismiss: None,
            debug_selector: None,
            root_style: StyleRefinement::default(),
        }
    }

    /// Replaces the controlled state captured by this render value.
    #[must_use]
    pub const fn controlled_state(mut self, state: DialogState) -> Self {
        self.state = state;
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
        handler: impl Fn(DialogDismissReason, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// Adds a stable selector prefix for the root and inspectable dialog parts.
    ///
    /// The root receives the supplied selector; backdrop, content, title,
    /// description, and close controls receive `-backdrop`, `-content`,
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
    pub const fn state(&self) -> DialogState {
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

    /// Returns the requested controlled value for a dismissal reason.
    #[must_use]
    pub const fn requested_dismissal(&self, reason: DialogDismissReason) -> Option<bool> {
        self.state.requested_dismissal(reason)
    }

    /// Returns the resolved theme/motion recipe.
    #[must_use]
    pub fn visual_style(&self) -> DialogStyle {
        DialogStyle::resolve(self.theme, self.motion)
    }

    /// Returns the retained title/description/close semantic metadata.
    #[must_use]
    pub fn semantics(&self) -> DialogSemantics {
        DialogSemantics {
            role: DIALOG_ROLE,
            title: self.title.clone(),
            description: self.description.clone(),
            close_label: SharedString::from(self.close_label.as_str().to_owned()),
        }
    }

    /// Returns the focus handle tracked by the built-in close control.
    ///
    /// Callers may use this as their entry target, or pass a different handle
    /// to [`DialogFocusIntent`] when the first meaningful control is elsewhere.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }
}

impl Styled for Dialog {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.root_style
    }
}

impl RenderOnce for Dialog {
    #[allow(clippy::too_many_lines)]
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if !self.state.is_open() {
            let mut closed = div()
                .id(ElementId::NamedChild(Box::new(self.id), "closed".into()))
                .absolute()
                .w(px(0.0))
                .h(px(0.0));
            if let Some(selector) = self.debug_selector {
                let selector = format!("{selector}-closed");
                closed = closed.debug_selector(move || selector);
            }
            return closed.into_any_element();
        }

        let Self {
            id,
            focus,
            theme,
            motion,
            title,
            description,
            content,
            close_label,
            on_dismiss,
            debug_selector,
            root_style,
            ..
        } = self;

        let style = DialogStyle::resolve(theme, motion);
        let viewport = window.viewport_size();
        let max_content = DialogGeometry::centered(
            viewport,
            size(style.max_width, viewport.height),
            style.max_width,
            None,
            style.viewport_margin,
        )
        .content
        .size;

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
        let backdrop_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-backdrop"));

        let mut title = div()
            .min_w_0()
            .text_size(style.title_size)
            .font_weight(style.title_weight)
            .text_color(style.foreground)
            .child(title);
        if let Some(selector) = title_selector {
            title = title.debug_selector(move || selector);
        }

        let mut header = div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(style.header_gap)
            .pr(style.close_button.width.unwrap_or(px(0.0)) + style.close_inset)
            .child(title);

        if let Some(description) = description {
            let mut description_element = div()
                .min_w_0()
                .text_size(style.description_size)
                .text_color(style.description_foreground)
                .child(description);
            if let Some(selector) = description_selector {
                description_element = description_element.debug_selector(move || selector);
            }
            header = header.child(description_element);
        }

        let content_shell = div().min_w_0().child(content);

        let close_dismiss = on_dismiss.clone();
        let mut close_button = Button::new(
            (id.clone(), "close"),
            focus,
            theme,
            motion,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(AssetId::TABLER_X, close_label),
        )
        .expect("the built-in icon-only close button has a valid label and size")
        .on_activate(move |event, window, cx| {
            if event.standard_click()
                && let Some(handler) = close_dismiss.as_ref()
            {
                handler(DialogDismissReason::CloseButton, window, cx);
            }
        });

        if let Some(selector) = close_selector {
            close_button = close_button.debug_selector(selector);
        }

        let close = div()
            .absolute()
            .top(style.close_inset)
            .right(style.close_inset)
            .child(close_button);

        let backdrop_dismiss = on_dismiss.clone();
        let mut backdrop = div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .left(px(0.0))
            .bg(style.overlay)
            .occlude()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                if let Some(handler) = backdrop_dismiss.as_ref() {
                    handler(DialogDismissReason::Backdrop, window, cx);
                }
                cx.stop_propagation();
            });

        if let Some(selector) = backdrop_selector {
            backdrop = backdrop.debug_selector(move || selector);
        }

        let mut content_frame = div()
            .relative()
            .flex()
            .flex_col()
            .min_w_0()
            .max_w(max_content.width)
            .max_h(max_content.height)
            .gap(style.content_gap)
            .p(style.padding)
            .rounded(style.corner_radius)
            .bg(style.background)
            .text_color(style.foreground)
            .shadow(vec![style.ring()])
            .overflow_hidden()
            .occlude()
            .child(header)
            .child(content_shell)
            .child(close);

        if let Some(selector) = selector
            .as_ref()
            .map(|selector| format!("{selector}-content"))
        {
            content_frame = content_frame.debug_selector(move || selector);
        }

        let escape_dismiss = on_dismiss;
        let mut root = div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .left(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .child(backdrop)
            .child(content_frame)
            .on_key_down(move |event, window, cx| {
                if event.keystroke.key.eq_ignore_ascii_case("escape")
                    && !event.keystroke.modifiers.modified()
                {
                    window.prevent_default();
                    if let Some(handler) = escape_dismiss.as_ref() {
                        handler(DialogDismissReason::Escape, window, cx);
                    }
                    cx.stop_propagation();
                }
            });

        root.style().refine(&root_style);
        if let Some(selector) = selector {
            root = root.debug_selector(move || selector);
        }

        match style.motion {
            DialogMotionPlan::Immediate => root.into_any_element(),
            DialogMotionPlan::Animate(animation) => root
                .with_animation((id, "open"), animation.gpui_clock(), |element, progress| {
                    element.opacity(progress)
                })
                .into_any_element(),
        }
    }
}
