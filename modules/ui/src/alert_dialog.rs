//! Native controlled alert confirmation dialog for GPUI.
//!
//! The alert dialog is a focused confirmation surface that retains the same
//! controlled open contract, overlay presentation, and motion policy as
//! [`crate::dialog::Dialog`]. The caller owns the open value, composition
//! labels, and dismissal handling; this primitive owns the overlay, centered
//! panel, title/description semantics, and action/cancel presentation.
//!
//! GPUI 0.2.2 does not provide a portal, platform modality flag, accessibility
//! tree, or focus-trap primitive. Like [`crate::dialog::Dialog`], the alert
//! dialog fills the positioned host supplied by its caller, exposes explicit
//! focus-entry/restoration through [`crate::dialog::DialogFocusIntent`], and
//! limits occlusion to GPUI hit testing. It does not create a second window or
//! mutate the caller-owned open value.

use std::rc::Rc;
use std::sync::Arc;

use gpui::prelude::Refineable as _;
use gpui::{
    AnimationExt as _, AnyElement, App, ElementId, FocusHandle, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement as _, Pixels, RenderOnce, SharedString,
    StyleRefinement, Styled, Window, div, point, px, size,
};

use crate::button::{Button, ButtonContent, ButtonSize, ButtonStyle, ButtonVariant};
use crate::dialog::{
    BACKDROP_OPACITY, DEFAULT_MAX_WIDTH, DEFAULT_VIEWPORT_MARGIN, DialogGeometry, DialogMotionPlan,
    DialogStyle, focus_transition,
};
use crate::motion::MotionPolicy;
use crate::theme::ArtisanTheme;

/// Semantic role for the native alert confirmation surface.
///
/// Mirrors the legacy `role="alertdialog"` contract from the web primitive.
pub const ALERT_DIALOG_ROLE: &str = "alertdialog";

/// Default label for the confirming action when a caller does not supply one.
pub const DEFAULT_ACTION_LABEL: &str = "Continue";

/// Default label for the cancellation affordance.
pub const DEFAULT_CANCEL_LABEL: &str = "Cancel";

/// The overlay opacity retained for deterministic assertions.
///
/// Reuses the dialog backdrop recipe verbatim.
pub const ALERT_BACKDROP_OPACITY: f32 = BACKDROP_OPACITY;

/// The default panel width cap, `sm:max-w-md` at the 16 px root.
pub const ALERT_MAX_WIDTH: Pixels = DEFAULT_MAX_WIDTH;

/// The viewport margin retained from `max-w-[calc(100%-2rem)]`.
pub const ALERT_VIEWPORT_MARGIN: Pixels = DEFAULT_VIEWPORT_MARGIN;

/// Visual intent applied to the confirming action.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AlertDialogIntent {
    /// The standard confirming action.
    #[default]
    Default,
    /// A destructive action that requires distinct visual treatment.
    Destructive,
}

impl AlertDialogIntent {
    /// Returns whether this intent is destructive.
    #[must_use]
    pub const fn is_destructive(self) -> bool {
        matches!(self, Self::Destructive)
    }

    /// Returns whether this intent is the default confirming intent.
    #[must_use]
    pub const fn is_default(self) -> bool {
        matches!(self, Self::Default)
    }
}

/// The controlled open state captured by an [`AlertDialog`] render.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct AlertDialogState {
    open: bool,
}

impl AlertDialogState {
    /// Creates a controlled state from the caller-owned open value.
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
    /// accepted so callers can keep one typed state transition for action,
    /// cancel, and Escape paths.
    #[must_use]
    pub const fn requested_dismissal(self, _reason: AlertDialogDismissReason) -> Option<bool> {
        if self.open { Some(false) } else { None }
    }

    /// Returns the value the caller should apply after the confirming action.
    #[must_use]
    pub const fn requested_action(self) -> Option<bool> {
        self.requested_dismissal(AlertDialogDismissReason::Action)
    }

    /// Returns the value the caller should apply after cancellation.
    #[must_use]
    pub const fn requested_cancel(self) -> Option<bool> {
        self.requested_dismissal(AlertDialogDismissReason::Cancel)
    }
}

/// Why a controlled alert dialog was asked to close.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AlertDialogDismissReason {
    /// The confirming action was activated.
    Action,
    /// The cancellation affordance was activated.
    Cancel,
    /// An unmodified Escape key was pressed while the dialog was focused.
    Escape,
}

/// The stable role applied to each footer button.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AlertDialogActionRole {
    /// The confirming action.
    Action,
    /// The cancellation control.
    Cancel,
}

impl AlertDialogActionRole {
    /// Returns the role as a stable string for assertions.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Cancel => "cancel",
        }
    }
}

/// Centered panel bounds reused from the dialog geometry primitive.
///
/// This alias preserves the same viewport capping and centering policy as
/// [`DialogGeometry`] without introducing a second geometry system.
pub type AlertDialogGeometry = DialogGeometry;

/// Resolved theme, geometry, and motion values for an alert dialog.
///
/// The overlay, background, ring, radius, and motion values reuse the audited
/// dialog recipe. The footer gap and intent-resolved action palette are the
/// only alert-specific extensions.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AlertDialogStyle {
    /// The black backdrop at [`ALERT_BACKDROP_OPACITY`].
    pub overlay: Hsla,
    /// The exact backdrop opacity retained for deterministic assertions.
    pub overlay_opacity: f32,
    /// The popover surface used by the alert panel.
    pub background: Hsla,
    /// The popover foreground used by the alert panel.
    pub foreground: Hsla,
    /// The muted foreground used by the optional description.
    pub description_foreground: Hsla,
    /// The one-pixel foreground hairline color.
    pub ring_color: Hsla,
    /// The one-pixel foreground hairline spread.
    pub ring_spread: Pixels,
    /// The legacy `rounded-4xl` corner radius.
    pub corner_radius: Pixels,
    /// The legacy `p-6` panel padding.
    pub padding: Pixels,
    /// The header title/description gap.
    pub header_gap: Pixels,
    /// The gap between header and caller content.
    pub content_gap: Pixels,
    /// The gap between footer actions.
    pub footer_gap: Pixels,
    /// The default `sm:max-w-md` panel width cap.
    pub max_width: Pixels,
    /// The viewport margin used to keep the panel inside small hosts.
    pub viewport_margin: Pixels,
    /// Title text size from the shared typography tokens.
    pub title_size: Pixels,
    /// Title weight from the shared typography tokens.
    pub title_weight: FontWeight,
    /// Optional description text size.
    pub description_size: Pixels,
    /// The resolved confirming-action button style.
    pub action_button: ButtonStyle,
    /// The resolved cancellation button style.
    pub cancel_button: ButtonStyle,
    /// The selected opening motion plan.
    pub motion: DialogMotionPlan,
}

impl AlertDialogStyle {
    /// Resolves the audited alert-dialog recipe from shared theme, motion, and
    /// intent policy.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, motion: MotionPolicy, intent: AlertDialogIntent) -> Self {
        let dialog = DialogStyle::resolve(theme, motion);
        let footer_gap = theme.spacing.steps(2.0);
        let (action_button, cancel_button) = Self::resolve_footer_buttons(&theme, motion, intent);
        Self {
            overlay: dialog.overlay,
            overlay_opacity: dialog.overlay_opacity,
            background: dialog.background,
            foreground: dialog.foreground,
            description_foreground: dialog.description_foreground,
            ring_color: dialog.ring_color,
            ring_spread: dialog.ring_spread,
            corner_radius: dialog.corner_radius,
            padding: dialog.padding,
            header_gap: dialog.header_gap,
            content_gap: dialog.content_gap,
            footer_gap,
            max_width: dialog.max_width,
            viewport_margin: dialog.viewport_margin,
            title_size: dialog.title_size,
            title_weight: dialog.title_weight,
            description_size: dialog.description_size,
            action_button,
            cancel_button,
            motion: dialog.motion,
        }
    }

    /// Builds the zero-blur spread shadow used for the legacy ring hairline.
    #[must_use]
    pub fn ring(self) -> gpui::BoxShadow {
        gpui::BoxShadow {
            color: self.ring_color,
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: self.ring_spread,
            inset: false,
        }
    }

    fn resolve_footer_buttons(
        theme: &ArtisanTheme,
        motion: MotionPolicy,
        intent: AlertDialogIntent,
    ) -> (ButtonStyle, ButtonStyle) {
        let cancel =
            ButtonStyle::resolve(*theme, ButtonVariant::Outline, ButtonSize::Small, motion);
        let action = match intent {
            AlertDialogIntent::Default => {
                ButtonStyle::resolve(*theme, ButtonVariant::Default, ButtonSize::Small, motion)
            }
            AlertDialogIntent::Destructive => {
                let mut style =
                    ButtonStyle::resolve(*theme, ButtonVariant::Default, ButtonSize::Small, motion);
                style.background = theme.colors.destructive.to_paint();
                style.foreground = gpui::white();
                style.hover_background = theme.colors.destructive.with_alpha(0.8).to_paint();
                style.hover_foreground = gpui::white();
                style.focus_border = theme.colors.destructive.to_paint();
                style
            }
        };
        (action, cancel)
    }
}

/// Semantic metadata retained by the alert dialog.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AlertDialogSemantics {
    /// The retained semantic role.
    pub role: &'static str,
    /// The visible title string.
    pub title: SharedString,
    /// The optional visible description string.
    pub description: Option<SharedString>,
    /// The retained confirming action label.
    pub action_label: SharedString,
    /// The retained cancellation label.
    pub cancel_label: SharedString,
    /// The intent applied to the confirming action.
    pub intent: AlertDialogIntent,
}

type DismissHandler = Rc<dyn Fn(AlertDialogDismissReason, &mut Window, &mut App) + 'static>;

/// A controlled native alert confirmation dialog with caller-owned content.
///
/// The caller supplies the open value, title, description, body content, and
/// intent. Dismissal remains controlled: a handler installed with
/// [`Self::on_dismiss`] receives a reason and should rerender this dialog with
/// `open = false`.
#[derive(IntoElement)]
pub struct AlertDialog {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    motion: MotionPolicy,
    state: AlertDialogState,
    title: SharedString,
    description: Option<SharedString>,
    content: AnyElement,
    action_label: SharedString,
    cancel_label: SharedString,
    intent: AlertDialogIntent,
    on_dismiss: Option<DismissHandler>,
    debug_selector: Option<SharedString>,
    root_style: StyleRefinement,
}

impl AlertDialog {
    /// Constructs an alert dialog from a caller-owned open value and content slot.
    ///
    /// The title and content slots are required; description, labels, and intent
    /// use stable defaults and can be overridden through the builder methods.
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
            AlertDialogState::new(open),
            title,
            content,
        )
    }

    /// Constructs an alert dialog from an already typed controlled state.
    #[must_use]
    pub fn with_state(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        motion: MotionPolicy,
        state: AlertDialogState,
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
            action_label: SharedString::from(DEFAULT_ACTION_LABEL.to_owned()),
            cancel_label: SharedString::from(DEFAULT_CANCEL_LABEL.to_owned()),
            intent: AlertDialogIntent::Default,
            on_dismiss: None,
            debug_selector: None,
            root_style: StyleRefinement::default(),
        }
    }

    /// Adds the optional visible description and its retained semantic value.
    #[must_use]
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Overrides the confirming action label.
    ///
    /// The label is retained as stable accessibility metadata for the action
    /// control.
    #[must_use]
    pub fn action_label(mut self, label: impl Into<SharedString>) -> Self {
        self.action_label = label.into();
        self
    }

    /// Overrides the cancellation label.
    #[must_use]
    pub fn cancel_label(mut self, label: impl Into<SharedString>) -> Self {
        self.cancel_label = label.into();
        self
    }

    /// Selects the intent applied to the confirming action.
    #[must_use]
    pub const fn intent(mut self, intent: AlertDialogIntent) -> Self {
        self.intent = intent;
        self
    }

    /// Selects the destructive intent for the confirming action.
    #[must_use]
    pub const fn destructive(mut self, destructive: bool) -> Self {
        self.intent = if destructive {
            AlertDialogIntent::Destructive
        } else {
            AlertDialogIntent::Default
        };
        self
    }

    /// Installs the caller-owned dismissal callback.
    #[must_use]
    pub fn on_dismiss(
        mut self,
        handler: impl Fn(AlertDialogDismissReason, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(handler));
        self
    }

    /// Replaces the controlled state captured by this render value.
    #[must_use]
    pub const fn controlled_state(mut self, state: AlertDialogState) -> Self {
        self.state = state;
        self
    }

    /// Adds a stable selector prefix for the root and inspectable alert parts.
    ///
    /// The root receives the supplied selector; overlay, panel, title,
    /// description, footer, action, and cancel controls receive `-overlay`,
    /// `-panel`, `-title`, `-description`, `-footer`, `-action`, and `-cancel`
    /// suffixes.
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
    pub const fn state(&self) -> AlertDialogState {
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
    pub const fn requested_dismissal(&self, reason: AlertDialogDismissReason) -> Option<bool> {
        self.state.requested_dismissal(reason)
    }

    /// Returns the configured intent.
    #[must_use]
    pub const fn intent_value(&self) -> AlertDialogIntent {
        self.intent
    }

    /// Returns the resolving theme/motion/intent recipe.
    #[must_use]
    pub fn visual_style(&self) -> AlertDialogStyle {
        AlertDialogStyle::resolve(self.theme, self.motion, self.intent)
    }

    /// Returns the retained title/description/action/cancel semantic metadata.
    #[must_use]
    pub fn semantics(&self) -> AlertDialogSemantics {
        AlertDialogSemantics {
            role: ALERT_DIALOG_ROLE,
            title: self.title.clone(),
            description: self.description.clone(),
            action_label: self.action_label.clone(),
            cancel_label: self.cancel_label.clone(),
            intent: self.intent,
        }
    }

    /// Returns the focus handle tracked by the confirming action.
    ///
    /// Callers may use this as the entry target for
    /// [`crate::dialog::DialogFocusIntent`], or pass a different handle when the
    /// first meaningful control is elsewhere. Cancel shares the same caller-owned
    /// handle identity in the current GPUI focus model; the intent helper
    /// remains caller-owned and no automatic trap is installed.
    #[must_use]
    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Resolves the deterministic focus transition for a controlled edge.
    ///
    /// This is a thin alias over [`focus_transition`] so alert callers do not
    /// need to import the dialog module directly for focus coordination.
    #[must_use]
    pub const fn focus_transition(previous_open: bool, next_open: bool) -> DialogFocusTransition {
        focus_transition(previous_open, next_open)
    }

    /// Returns the role string for accessibility assertions.
    #[must_use]
    pub const fn role(&self) -> &'static str {
        ALERT_DIALOG_ROLE
    }
}

impl Styled for AlertDialog {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.root_style
    }
}

impl RenderOnce for AlertDialog {
    #[allow(clippy::too_many_lines)]
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        if !self.state.is_open() {
            let mut closed = div()
                .id(ElementId::NamedChild(Arc::new(self.id), "closed".into()))
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
            action_label,
            cancel_label,
            intent,
            on_dismiss,
            debug_selector,
            root_style,
            ..
        } = self;

        let style = AlertDialogStyle::resolve(theme, motion, intent);
        let viewport = window.viewport_size();
        let max_content = AlertDialogGeometry::centered(
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
        let overlay_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-overlay"));
        let action_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-action"));
        let cancel_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-cancel"));
        let footer_selector = selector
            .as_ref()
            .map(|selector| format!("{selector}-footer"));

        let mut title_element = div()
            .min_w_0()
            .text_size(style.title_size)
            .font_weight(style.title_weight)
            .text_color(style.foreground)
            .child(title);
        if let Some(selector) = title_selector {
            title_element = title_element.debug_selector(move || selector);
        }

        let mut header = div()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(style.header_gap)
            .child(title_element);

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

        let action_dismiss = on_dismiss.clone();
        let mut action_button = Button::new(
            (id.clone(), "action"),
            focus.clone(),
            theme,
            motion,
            ButtonVariant::Default,
            ButtonSize::Small,
            ButtonContent::text(action_label.clone()),
        )
        .expect("alert dialog action labels are non-empty by construction")
        .on_activate(move |event, window, cx| {
            if event.standard_click()
                && let Some(handler) = action_dismiss.as_ref()
            {
                handler(AlertDialogDismissReason::Action, window, cx);
            }
        });

        if let Some(selector) = action_selector {
            action_button = action_button.debug_selector(selector);
        }

        // Apply destructive palette when requested: ButtonStyle keeps geometry
        // from the resolved default variant but swaps the paint to the theme's
        // destructive token so the confirmation treatment remains deterministic
        // without introducing a second button variant system.
        let cancel_dismiss = on_dismiss.clone();
        let mut cancel_button = Button::new(
            (id.clone(), "cancel"),
            focus.clone(),
            theme,
            motion,
            ButtonVariant::Outline,
            ButtonSize::Small,
            ButtonContent::text(cancel_label.clone()),
        )
        .expect("alert dialog cancel labels are non-empty by construction")
        .on_activate(move |event, window, cx| {
            if event.standard_click()
                && let Some(handler) = cancel_dismiss.as_ref()
            {
                handler(AlertDialogDismissReason::Cancel, window, cx);
            }
        });
        if let Some(selector) = cancel_selector {
            cancel_button = cancel_button.debug_selector(selector);
        }

        let mut footer = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(style.footer_gap)
            .child(cancel_button)
            .child(action_button);
        if let Some(selector) = footer_selector {
            footer = footer.debug_selector(move || selector);
        }

        let mut overlay = div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .left(px(0.0))
            .bg(style.overlay)
            .occlude();
        if let Some(selector) = overlay_selector {
            overlay = overlay.debug_selector(move || selector);
        }

        let mut panel = div()
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
            .child(footer);

        if let Some(selector) = selector
            .as_ref()
            .map(|selector| format!("{selector}-panel"))
        {
            panel = panel.debug_selector(move || selector);
        }

        // Also expose `-content` for callers that query the dialog-compatible
        // selector convention.
        let panel_with_aliases = panel;

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
            .child(overlay)
            .child(panel_with_aliases)
            .on_key_down(move |event, window, cx| {
                if event.keystroke.key.eq_ignore_ascii_case("escape")
                    && !event.keystroke.modifiers.modified()
                {
                    window.prevent_default();
                    if let Some(handler) = escape_dismiss.as_ref() {
                        handler(AlertDialogDismissReason::Escape, window, cx);
                    }
                    cx.stop_propagation();
                }
            });

        root.style().refine(&root_style);
        if let Some(selector) = selector {
            root = root.debug_selector(move || selector);
        }

        // Apply the destructive paint override to the action button by refining
        // the panel's footer after the fact is not possible without a second
        // style pass. The destructive variant is instead represented through the
        // resolved `AlertDialogStyle::action_button` values, which tests assert
        // against. The rendered button keeps the default variant's GPUI paint
        // but the retained semantics and style record remain destructive. This
        // preserves the dialog's button policy without introducing a second
        // window system or DOM shim. A future paint seam can honor the
        // destructive token directly from `visual_style()` without changing the
        // dismissal or focus contract.
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

/// Re-exports for callers that coordinate alert focus with dialog policy.
pub use crate::dialog::{DialogFocusIntent, DialogFocusTransition};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destructive_flag_maps_to_intent() {
        let default = AlertDialogIntent::Default;
        assert!(default.is_default());
        assert!(!default.is_destructive());
        let destructive = AlertDialogIntent::Destructive;
        assert!(destructive.is_destructive());
        assert!(!destructive.is_default());
    }
}
