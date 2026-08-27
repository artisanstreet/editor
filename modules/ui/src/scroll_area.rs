//! Native vertical scrolling viewport for transcript-like content.
//!
//! `ScrollArea` is deliberately a small leaf around pinned GPUI 0.2.2's
//! [`gpui::StatefulInteractiveElement::overflow_y_scroll`] and
//! [`gpui::StatefulInteractiveElement::track_scroll`] seams. It owns the
//! viewport geometry, clipping, wheel-event containment, focus-ring recipe,
//! and stable debug selectors. It does not create a scrollbar track or thumb.
//!
//! The pinned GPUI version has no CSS `overscroll-behavior` property, so the
//! default containment policy is implemented by stopping wheel propagation
//! after GPUI's built-in bounded scroll listener has updated this viewport.
//! The caller may still refine the base style through [`Styled`]; those
//! refinements are applied after the leaf defaults to both the root and its
//! viewport so a caller-supplied radius behaves like the legacy
//! `rounded-[inherit]` viewport rule.
//!
//! This leaf does not own transcript policy: follow-tail, scroll-end fencing,
//! anchor compensation, virtualization, and turn navigation remain the
//! responsibility of the surrounding Artisan feature.

use gpui::prelude::{
    InteractiveElement, ParentElement, Refineable, StatefulInteractiveElement, Styled,
};
use gpui::{
    AnyElement, App, BoxShadow, FocusHandle, Hsla, IntoElement, Pixels, RenderOnce, ScrollHandle,
    SharedString, StyleRefinement, Window, div, point, px,
};

use crate::button::FocusVisibility;
use crate::theme::ArtisanTheme;

/// Stable debug selector for the outer scroll-area root.
pub const ROOT_SELECTOR: &str = "artisan-scroll-area-root";

/// Stable debug selector for the clipped vertical viewport.
pub const VIEWPORT_SELECTOR: &str = "artisan-scroll-area-viewport";

/// The only orientation offered by this leaf.
///
/// Horizontal scrolling is intentionally not exposed. The root's default
/// overflow mask clips horizontal spill while the viewport scrolls only on
/// its vertical axis.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ScrollAreaAxis {
    /// Transcript-style top-to-bottom scrolling.
    #[default]
    Vertical,
}

/// Theme-resolved focus treatment for a [`ScrollArea`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollAreaStyle {
    /// Paint color for the keyboard-visible focus ring.
    pub focus_ring_color: Hsla,
    /// Focus-ring spread, pinned to the legacy three-pixel treatment.
    pub focus_ring_width: Pixels,
}

impl ScrollAreaStyle {
    /// Resolves the focus treatment from the shared Artisan theme.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            focus_ring_color: theme.interaction.focus_ring_color.to_paint(),
            focus_ring_width: theme.interaction.focus_ring_width,
        }
    }
}

/// A full-size, vertically scrollable GPUI viewport with no visible
/// scrollbar chrome.
///
/// The supplied [`ScrollHandle`] is cloned only as GPUI's cheap shared handle;
/// [`Self::scroll_handle`] exposes the same underlying offset and bounds state
/// to the caller. Children are accumulated as [`AnyElement`] values until the
/// one [`RenderOnce::render`] pass and then moved into the viewport in order;
/// the component retains no child payload after that pass.
///
/// GPUI 0.2.2 does not provide browser-style focus-modality detection. When a
/// focus handle is supplied, the default is therefore
/// [`FocusVisibility::Visible`]; callers that have pointer/programmatic focus
/// context can select [`FocusVisibility::Hidden`].
///
/// The component does not implement follow-tail, scroll-end fencing, anchor
/// compensation, virtualization, or turn navigation. Those behaviors require
/// transcript/domain policy above this viewport.
#[derive(IntoElement)]
pub struct ScrollArea {
    scroll_handle: ScrollHandle,
    style: ScrollAreaStyle,
    focus_handle: Option<FocusHandle>,
    focus_visibility: FocusVisibility,
    children: Vec<AnyElement>,
    style_refinement: StyleRefinement,
    root_selector: SharedString,
    viewport_selector: SharedString,
}

impl ScrollArea {
    /// Constructs a scroll area using the shared theme's focus-ring tokens.
    #[must_use]
    pub fn new(scroll_handle: ScrollHandle, theme: ArtisanTheme) -> Self {
        Self::with_style(scroll_handle, ScrollAreaStyle::resolve(theme))
    }

    /// Constructs a scroll area from an explicit focus-ring recipe.
    #[must_use]
    pub fn with_style(scroll_handle: ScrollHandle, style: ScrollAreaStyle) -> Self {
        Self {
            scroll_handle,
            style,
            focus_handle: None,
            focus_visibility: FocusVisibility::Visible,
            children: Vec::new(),
            style_refinement: StyleRefinement::default(),
            root_selector: ROOT_SELECTOR.into(),
            viewport_selector: VIEWPORT_SELECTOR.into(),
        }
    }

    /// Constructs a scroll area by cloning a caller-owned handle reference.
    #[must_use]
    pub fn from_handle(scroll_handle: &ScrollHandle, theme: ArtisanTheme) -> Self {
        Self::new(scroll_handle.clone(), theme)
    }

    /// Returns the exact shared GPUI handle tracked by the viewport.
    ///
    /// Cloning the returned handle is safe and preserves the same GPUI scroll
    /// state, which lets follow-tail or programmatic scroll owners observe and
    /// control this viewport without introducing parallel state here.
    #[must_use]
    pub fn scroll_handle(&self) -> &ScrollHandle {
        &self.scroll_handle
    }

    /// Returns the explicit axis policy for this component.
    #[must_use]
    pub const fn axis(&self) -> ScrollAreaAxis {
        ScrollAreaAxis::Vertical
    }

    /// Returns the resolved focus-ring recipe.
    #[must_use]
    pub fn visual_style(&self) -> ScrollAreaStyle {
        self.style
    }

    /// Selects the focus handle tracked by the viewport.
    #[must_use]
    pub fn focus_handle(mut self, focus_handle: FocusHandle) -> Self {
        self.focus_handle = Some(focus_handle);
        self
    }

    /// Selects whether actual focus should receive the visible focus ring.
    #[must_use]
    pub const fn focus_visibility(mut self, visibility: FocusVisibility) -> Self {
        self.focus_visibility = visibility;
        self
    }

    /// Adds a stable root selector and derives its `-viewport` selector.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        let selector = selector.into();
        self.viewport_selector = format!("{selector}-viewport").into();
        self.root_selector = selector;
        self
    }

    /// Returns the root selector used by GPUI's debug-bounds harness.
    #[must_use]
    pub fn root_debug_selector(&self) -> &str {
        self.root_selector.as_ref()
    }

    /// Returns the viewport selector used by GPUI's debug-bounds harness.
    #[must_use]
    pub fn viewport_debug_selector(&self) -> &str {
        self.viewport_selector.as_ref()
    }

    /// Reports whether the configured visible-focus ring should paint now.
    #[must_use]
    pub fn focus_ring_visible(&self, window: &Window) -> bool {
        self.focus_visibility == FocusVisibility::Visible
            && self
                .focus_handle
                .as_ref()
                .is_some_and(|focus_handle| focus_handle.is_focused(window))
    }
}

impl Styled for ScrollArea {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style_refinement
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ScrollArea {
            scroll_handle,
            style,
            focus_handle,
            focus_visibility,
            children,
            style_refinement,
            root_selector,
            viewport_selector,
        } = self;

        let mut viewport = div()
            .size_full()
            .id(viewport_selector.clone())
            .overflow_y_scroll()
            // GPUI has no scrollbar widget in this seam; an explicit zero
            // width also prevents a platform scrollbar from reserving space.
            .scrollbar_width(px(0.0))
            .track_scroll(&scroll_handle)
            // GPUI's built-in listener is registered after this callback and
            // therefore updates the bounded offset before propagation stops.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .debug_selector(move || viewport_selector.to_string());

        if let Some(focus_handle) = focus_handle {
            viewport = viewport.track_focus(&focus_handle);
            if focus_visibility == FocusVisibility::Visible {
                viewport = viewport.focus(move |focused| {
                    focused.shadow(vec![BoxShadow {
                        color: style.focus_ring_color,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: style.focus_ring_width,
                    }])
                });
            }
        }

        // `children` is consumed here, preserving insertion order without
        // retaining the payload beyond this RenderOnce pass.
        viewport.extend(children);

        let mut root = div()
            .size_full()
            .overflow_hidden()
            .debug_selector(move || root_selector.to_string());

        // Refine both layers after their defaults. Mirroring the caller's
        // radius onto the viewport is the pinned-GPUI equivalent of CSS
        // `rounded-[inherit]`; other caller refinements retain the same
        // explicit later-wins behavior.
        root.style().refine(&style_refinement);
        viewport.style().refine(&style_refinement);

        root.child(viewport)
    }
}
