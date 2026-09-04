//! Controlled native GPUI toggle primitive.
//!
//! A standalone pressed toggle distinct from [`crate::switch::Switch`] and
//! [`crate::toggle_group::ToggleGroup`]: the pressed value remains owned by the
//! caller, and an activation produces the next controlled value without mutating
//! retained state. Visuals follow the reached web recipe (`toggle.svelte`) and
//! shared theme tokens; activation is pointer click plus GPUI's synthesized
//! `Enter`/`Space` keyboard clicks.

use artisan_assets::AssetId;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, BoxShadow, ClickEvent, Div, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, div, point, px, transparent_black,
};
use thiserror::Error;

use crate::asset_seam::asset_glyph;
use crate::button::{AccessibleLabel, FocusVisibility};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};

const CONTENT_GAP_PX: f32 = 4.0;
const ICON_SIZE_PX: f32 = 16.0;
const DISABLED_OPACITY: f32 = 0.5;

/// Visual variant reached by the toggle recipe.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ToggleVariant {
    /// Transparent resting background; pressed uses muted.
    #[default]
    Default,
    /// Adds the shared input border; hover and pressed use muted.
    Outline,
}

/// Toggle dimensions on the shared control scale.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ToggleSize {
    /// `h-9` with `min-w-9` and `px-3`.
    #[default]
    Default,
    /// `h-8` with `min-w-8` and `px-3`.
    Small,
    /// `h-10` with `min-w-10` and `px-4`.
    Large,
}

/// Typed composition for a toggle's visible content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToggleContent {
    /// Visible text supplies the accessible name.
    Text(SharedString),
    /// One catalog icon with mandatory retained accessibility metadata.
    IconOnly {
        /// Sealed catalog identifier routed by the existing asset seam.
        icon: AssetId,
        /// Explicit non-empty accessible name.
        accessible_label: AccessibleLabel,
    },
    /// A catalog icon followed by visible text.
    IconText {
        /// Sealed catalog identifier routed by the existing asset seam.
        icon: AssetId,
        /// Visible label, which also supplies the accessible name.
        label: SharedString,
    },
}

impl ToggleContent {
    /// Constructs visible text content.
    #[must_use]
    pub fn text(label: impl Into<SharedString>) -> Self {
        Self::Text(label.into())
    }

    /// Constructs an icon-only toggle from a sealed asset and validated name.
    #[must_use]
    pub const fn icon_only(icon: AssetId, accessible_label: AccessibleLabel) -> Self {
        Self::IconOnly {
            icon,
            accessible_label,
        }
    }

    /// Constructs an icon followed by visible text.
    #[must_use]
    pub fn icon_text(icon: AssetId, label: impl Into<SharedString>) -> Self {
        Self::IconText {
            icon,
            label: label.into(),
        }
    }

    /// Returns the visible or explicitly retained accessible name.
    #[must_use]
    pub fn accessible_label(&self) -> &str {
        match self {
            Self::Text(label) | Self::IconText { label, .. } => label.as_ref(),
            Self::IconOnly {
                accessible_label, ..
            } => accessible_label.as_str(),
        }
    }

    const fn is_icon_only(&self) -> bool {
        matches!(self, Self::IconOnly { .. })
    }
}

/// An incompatible toggle content construction was requested.
#[derive(Clone, Copy, Debug, Eq, Error, Hash, PartialEq)]
pub enum ToggleConstructionError {
    /// Visible text must supply an accessible name.
    #[error("a visible toggle label cannot be empty or whitespace-only")]
    EmptyVisibleLabel,
}

/// Theme-resolved geometry and paint for one toggle configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToggleStyle {
    /// Control height.
    pub height: Pixels,
    /// Minimum control width.
    pub min_width: Pixels,
    /// Horizontal padding inside the control.
    pub horizontal_padding: Pixels,
    /// Gap between an icon and adjacent text.
    pub content_gap: Pixels,
    /// Default icon edge.
    pub icon_size: Pixels,
    /// Rounded pill radius from the theme radius ramp.
    pub corner_radius: Pixels,
    /// Resting background (unpressed).
    pub background: Hsla,
    /// Resting text and icon color.
    pub foreground: Hsla,
    /// Resting one-pixel border color.
    pub border: Hsla,
    /// Pointer-hover background (unpressed).
    pub hover_background: Hsla,
    /// Pointer-hover text and icon color (unpressed).
    pub hover_foreground: Hsla,
    /// Pressed background.
    pub pressed_background: Hsla,
    /// Pressed text and icon color.
    pub pressed_foreground: Hsla,
    /// Keyboard-visible focus-ring color.
    pub focus_ring: Hsla,
    /// Full-alpha keyboard-visible focus-border color.
    pub focus_border: Hsla,
    /// Keyboard-visible focus-ring spread.
    pub focus_ring_width: Pixels,
    /// Disabled opacity retained from the legacy wrapper.
    pub disabled_opacity: f32,
    /// Text size.
    pub text_size: Pixels,
}

impl ToggleStyle {
    /// Resolves the exact toggle recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(
        theme: ArtisanTheme,
        variant: ToggleVariant,
        size: ToggleSize,
        pressed: bool,
    ) -> Self {
        let (height, min_width, horizontal_padding) = match size {
            ToggleSize::Default => (
                theme.density.control_default,
                theme.density.control_default,
                px(12.0),
            ),
            ToggleSize::Small => (theme.density.control_sm, theme.density.control_sm, px(12.0)),
            ToggleSize::Large => (theme.density.control_lg, theme.density.control_lg, px(16.0)),
        };

        let transparent = transparent_black();
        let border = match variant {
            ToggleVariant::Default => transparent,
            ToggleVariant::Outline => theme.colors.input.to_paint(),
        };

        let hover_background = match theme.mode {
            ThemeMode::Light => theme.colors.muted.to_paint(),
            ThemeMode::Dark => theme.colors.muted.with_alpha(0.5).to_paint(),
        };

        let pressed_background = theme.colors.muted.to_paint();
        let pressed_foreground = theme.colors.foreground.to_paint();

        // Background for unpressed is always transparent; pressed uses muted.
        // The `pressed` flag does not change the resolved record's `background`
        // field, but callers can branch on it; the resolved `pressed_background`
        // is always available. Keep the record deterministic for the given
        // `pressed` so tests can assert exact paints.
        let background = if pressed {
            pressed_background
        } else {
            transparent
        };

        Self {
            height,
            min_width,
            horizontal_padding,
            content_gap: px(CONTENT_GAP_PX),
            icon_size: px(ICON_SIZE_PX),
            corner_radius: RadiusTokens::value(RadiusStep::X4l),
            background,
            foreground: theme.colors.foreground.to_paint(),
            border,
            hover_background,
            hover_foreground: theme.colors.foreground.to_paint(),
            pressed_background,
            pressed_foreground,
            focus_ring: theme.interaction.focus_ring_color.to_paint(),
            focus_border: theme.colors.ring.to_paint(),
            focus_ring_width: theme.interaction.focus_ring_width,
            disabled_opacity: DISABLED_OPACITY,
            text_size: theme.typography.control_text,
        }
    }
}

/// Pure, deterministic next pressed value for a toggle activation.
///
/// Disabled toggles never change; the caller retains control of when to apply
/// the returned value. No DOM or window state is consulted.
///
/// # Examples
///
/// ```
/// # use artisan_ui::toggle::next_pressed;
/// assert_eq!(next_pressed(false, false), true);
/// assert_eq!(next_pressed(true, false), false);
/// assert_eq!(next_pressed(false, true), false);
/// ```
#[must_use]
pub const fn next_pressed(pressed: bool, disabled: bool) -> bool {
    if disabled { pressed } else { !pressed }
}

/// Alias for [`next_pressed`] without the disabled guard.
#[must_use]
pub const fn toggled(pressed: bool) -> bool {
    !pressed
}

type ChangeHandler = Box<dyn Fn(bool, &ClickEvent, &mut Window, &mut App)>;

/// A controlled, focusable native toggle with pressed state.
#[derive(IntoElement)]
pub struct Toggle {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    variant: ToggleVariant,
    size: ToggleSize,
    pressed: bool,
    disabled: bool,
    focus_visibility: FocusVisibility,
    content: ToggleContent,
    on_change: Option<ChangeHandler>,
    root: Div,
    debug_selector: Option<SharedString>,
}

impl Toggle {
    /// Constructs a controlled toggle after validating visible content.
    ///
    /// # Errors
    ///
    /// Returns [`ToggleConstructionError::EmptyVisibleLabel`] when visible text
    /// content is empty or whitespace-only.
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        variant: ToggleVariant,
        size: ToggleSize,
        content: ToggleContent,
        pressed: bool,
    ) -> Result<Self, ToggleConstructionError> {
        if !content.is_icon_only() && content.accessible_label().trim().is_empty() {
            return Err(ToggleConstructionError::EmptyVisibleLabel);
        }

        Ok(Self {
            id: id.into(),
            focus,
            theme,
            variant,
            size,
            pressed,
            disabled: false,
            focus_visibility: FocusVisibility::Hidden,
            content,
            on_change: None,
            root: div(),
            debug_selector: None,
        })
    }

    /// Selects the disabled presentation and suppresses every interaction.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Selects whether actual focus should receive a visible ring.
    #[must_use]
    pub const fn focus_visibility(mut self, visibility: FocusVisibility) -> Self {
        self.focus_visibility = visibility;
        self
    }

    /// Installs the callback with the next controlled pressed value.
    #[must_use]
    pub fn on_pressed_change(
        mut self,
        handler: impl Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }

    /// Alias for [`Self::on_pressed_change`].
    #[must_use]
    pub fn on_change(
        self,
        handler: impl Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_pressed_change(handler)
    }

    /// Adds a stable selector for GPUI inspection and behavior tests.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the retained accessible name.
    #[must_use]
    pub fn accessible_label(&self) -> &str {
        self.content.accessible_label()
    }

    /// Returns the controlled pressed value retained by this render recipe.
    #[must_use]
    pub const fn pressed(&self) -> bool {
        self.pressed
    }

    /// Returns the controlled pressed value.
    #[must_use]
    pub const fn is_pressed(&self) -> bool {
        self.pressed
    }

    /// Returns whether this toggle is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns the resolved theme recipe for this configuration.
    #[must_use]
    pub fn visual_style(&self) -> ToggleStyle {
        ToggleStyle::resolve(self.theme, self.variant, self.size, self.pressed)
    }

    /// Whether this toggle should paint its keyboard-visible focus ring now.
    #[must_use]
    pub fn focus_ring_visible(&self, window: &Window) -> bool {
        !self.disabled
            && self.focus_visibility == FocusVisibility::Visible
            && self.focus.is_focused(window)
    }
}

impl Styled for Toggle {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

impl RenderOnce for Toggle {
    fn render(mut self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        let style = ToggleStyle::resolve(self.theme, self.variant, self.size, self.pressed);
        let disabled = self.disabled;
        let focus_visibility = self.focus_visibility;
        let focus = self.focus.clone();
        let pressed = self.pressed;
        let on_change = self.on_change;

        // Preserve any caller-provided size refinements before applying the
        // themed frame. Only the height/width overrides are preserved explicitly;
        // other refinements (gap, bg) are layered after the base so callers can
        // still override via `Styled`.
        let custom_height = self.root.style().size.height;
        let custom_width = self.root.style().size.width;

        let mut root = self.root;
        root = root
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .px(style.horizontal_padding)
            .gap(style.content_gap)
            .rounded(style.corner_radius)
            .border_1()
            .border_color(style.border)
            .bg(if pressed {
                style.pressed_background
            } else {
                style.background
            })
            .text_color(if pressed {
                style.pressed_foreground
            } else {
                style.foreground
            })
            .text_size(style.text_size)
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when(disabled, |element| element.opacity(style.disabled_opacity))
            .when_some(self.debug_selector, |element, selector| {
                element.debug_selector(move || selector.to_string())
            });

        // Apply themed height/min-width only when the caller did not already set
        // an explicit size. This mirrors the `Switch` probe's ability to
        // override the track size via `Styled`.
        if custom_height.is_none() {
            root = root.h(style.height);
        } else {
            root.style().size.height = custom_height;
        }
        if custom_width.is_none() {
            root = root.min_w(style.min_width);
        } else {
            root.style().size.width = custom_width;
        }

        let mut root = root.id(self.id);
        root = match self.content {
            ToggleContent::Text(label) => root.child(label),
            ToggleContent::IconOnly { icon, .. } => {
                root.child(asset_glyph(icon).size(style.icon_size))
            }
            ToggleContent::IconText { icon, label } => root
                .child(asset_glyph(icon).size(style.icon_size))
                .child(label),
        };

        if disabled {
            return root;
        }

        root = root
            .when(focus_visibility == FocusVisibility::Visible, |element| {
                element.focus(move |focused| {
                    focused
                        .border_color(style.focus_border)
                        .shadow(vec![BoxShadow {
                            color: style.focus_ring,
                            offset: point(px(0.0), px(0.0)),
                            blur_radius: px(0.0),
                            spread_radius: style.focus_ring_width,
                            inset: false,
                        }])
                })
            })
            .hover(move |hover| {
                hover
                    .bg(if pressed {
                        style.pressed_background
                    } else {
                        style.hover_background
                    })
                    .text_color(if pressed {
                        style.pressed_foreground
                    } else {
                        style.hover_foreground
                    })
            })
            .track_focus(&focus);

        if let Some(handler) = on_change {
            let next = toggled(pressed);
            root = root.on_click(move |event, window, cx| handler(next, event, window, cx));
        }

        root
    }
}
