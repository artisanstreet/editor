//! Native button primitive for the first Artisan conversation workflow.
//!
//! The public surface is intentionally smaller than the legacy Svelte wrapper:
//! only variants and sizes reached by the composer, transcript controls,
//! attachments, and question answers are represented. GPUI
//! supplies focus tracking and synthesizes Enter/Space clicks for a focused
//! element; this component owns the typed visual recipe, disabled suppression,
//! catalog-backed icon composition, and retained accessibility intent.

use artisan_assets::AssetId;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, BoxShadow, ClickEvent, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement,
    Styled, Window, div, point, px, transparent_black,
};
use thiserror::Error;

use crate::asset_seam::asset_glyph;
use crate::motion::MotionPolicy;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};

/// Button faces reached by the approved first native workflow.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ButtonVariant {
    /// Primary action: submit or answer.
    #[default]
    Default,
    /// Bordered alternative used by selectable answers.
    Outline,
    /// Quiet filled action used by attachment controls.
    Secondary,
    /// Chrome-free action used by composer and transcript controls.
    Ghost,
}

/// Button dimensions reached by the approved first native workflow.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ButtonSize {
    /// 32 px high, used for compact text and icon/text actions.
    #[default]
    Small,
    /// 32 px square, used for the composer send/stop control.
    IconSmall,
}

/// Whether the owner determined that current focus should be visibly painted.
///
/// GPUI 0.2.2 exposes focus handles but no browser-style `:focus-visible`
/// input-modality heuristic. The application focus coordinator therefore
/// supplies this explicit decision. The button still gates the ring on its
/// actual [`FocusHandle`] being focused, so visible metadata alone cannot
/// paint a ring on an unfocused control.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum FocusVisibility {
    /// Pointer/programmatic focus without a visible ring.
    #[default]
    Hidden,
    /// Keyboard-visible focus.
    Visible,
}

/// A non-empty accessible name retained for an icon-only control.
///
/// Pinned GPUI does not expose a complete accessibility tree, so this value is
/// first-class metadata for tests and future platform accessibility wiring.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AccessibleLabel(SharedString);

impl AccessibleLabel {
    /// Validates and retains an explicit accessible name.
    ///
    /// # Errors
    ///
    /// Returns [`AccessibleLabelError`] when `label` is empty or contains only
    /// whitespace.
    pub fn new(label: impl Into<SharedString>) -> Result<Self, AccessibleLabelError> {
        let label = label.into();
        if label.trim().is_empty() {
            return Err(AccessibleLabelError);
        }
        Ok(Self(label))
    }

    /// Returns the retained accessible name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0.as_ref()
    }
}

/// An icon-only control was given no usable accessible name.
#[derive(Clone, Copy, Debug, Eq, Error, Hash, PartialEq)]
#[error("an icon-only button requires a non-empty accessible label")]
pub struct AccessibleLabelError;

/// Typed content composition supported by the first workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ButtonContent {
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

impl ButtonContent {
    /// Constructs a visible text button.
    #[must_use]
    pub fn text(label: impl Into<SharedString>) -> Self {
        Self::Text(label.into())
    }

    /// Constructs an icon-only button from a sealed asset and validated name.
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

/// An incompatible typed size/content pairing was requested.
#[derive(Clone, Copy, Debug, Eq, Error, Hash, PartialEq)]
pub enum ButtonConstructionError {
    /// Visible text must supply the button's accessible name.
    #[error("a visible button label cannot be empty or whitespace-only")]
    EmptyVisibleLabel,
    /// The square size is exclusively for icon-only content.
    #[error("the icon-small button size requires icon-only content")]
    IconSizeRequiresIconOnly,
    /// Icon-only content must use the square size in this first-workflow API.
    #[error("icon-only button content requires the icon-small size")]
    IconOnlyRequiresIconSize,
}

/// Paint and geometry values resolved from one typed button configuration.
///
/// This record is public so behavioral tests and the future component gallery
/// can compare exact native semantics without reaching into GPUI internals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ButtonStyle {
    /// Control height.
    pub height: Pixels,
    /// Control width when this is a square icon button.
    pub width: Option<Pixels>,
    /// Horizontal padding for non-square content.
    pub horizontal_padding: Pixels,
    /// Gap between a catalog icon and visible text.
    pub content_gap: Pixels,
    /// Default icon edge.
    pub icon_size: Pixels,
    /// Rounded-pill radius from the theme radius ramp.
    pub corner_radius: Pixels,
    /// Resting background color.
    pub background: Hsla,
    /// Resting text/icon color.
    pub foreground: Hsla,
    /// Resting one-pixel border color.
    pub border: Hsla,
    /// Pointer-hover background.
    pub hover_background: Hsla,
    /// Pointer-hover text/icon color.
    pub hover_foreground: Hsla,
    /// Keyboard-visible focus-ring color.
    pub focus_ring: Hsla,
    /// Full-alpha keyboard-visible focus-border color.
    pub focus_border: Hsla,
    /// Keyboard-visible focus-ring spread.
    pub focus_ring_width: Pixels,
    /// Disabled opacity retained from the legacy wrapper.
    pub disabled_opacity: f32,
    /// Full-motion active offset; absent under reduced motion.
    pub pressed_offset_y: Option<Pixels>,
}

impl ButtonStyle {
    /// Resolves the exact first-workflow recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(
        theme: ArtisanTheme,
        variant: ButtonVariant,
        size: ButtonSize,
        motion: MotionPolicy,
    ) -> Self {
        let (height, width, horizontal_padding, content_gap) = match size {
            ButtonSize::Small => (theme.density.control_sm, None, px(12.0), px(4.0)),
            ButtonSize::IconSmall => (
                theme.density.control_sm,
                Some(theme.density.control_sm),
                px(0.0),
                px(0.0),
            ),
        };

        let transparent = transparent_black();
        let (background, foreground, border, hover_background, hover_foreground) = match variant {
            ButtonVariant::Default => (
                theme.colors.primary.to_paint(),
                theme.colors.primary_foreground.to_paint(),
                transparent,
                theme.colors.primary.with_alpha(0.8).to_paint(),
                theme.colors.primary_foreground.to_paint(),
            ),
            ButtonVariant::Outline => {
                let surface = match theme.mode {
                    ThemeMode::Light => SurfaceStep::S100,
                    ThemeMode::Dark => SurfaceStep::S900,
                };
                (
                    theme.surfaces.value(surface).to_paint(),
                    theme.colors.foreground.to_paint(),
                    theme.colors.border.to_paint(),
                    theme.colors.input.with_alpha(0.5).to_paint(),
                    theme.colors.foreground.to_paint(),
                )
            }
            ButtonVariant::Secondary => (
                theme.colors.secondary.to_paint(),
                theme.colors.secondary_foreground.to_paint(),
                transparent,
                theme.colors.secondary.with_alpha(0.8).to_paint(),
                theme.colors.secondary_foreground.to_paint(),
            ),
            ButtonVariant::Ghost => {
                let hover = match theme.mode {
                    ThemeMode::Light => theme.colors.muted,
                    ThemeMode::Dark => theme.colors.muted.with_alpha(0.5),
                };
                (
                    transparent,
                    theme.colors.foreground.to_paint(),
                    transparent,
                    hover.to_paint(),
                    theme.colors.foreground.to_paint(),
                )
            }
        };

        Self {
            height,
            width,
            horizontal_padding,
            content_gap,
            icon_size: px(16.0),
            corner_radius: RadiusTokens::value(RadiusStep::X4l),
            background,
            foreground,
            border,
            hover_background,
            hover_foreground,
            focus_ring: theme.interaction.focus_ring_color.to_paint(),
            focus_border: theme.colors.ring.to_paint(),
            focus_ring_width: theme.interaction.focus_ring_width,
            disabled_opacity: 0.5,
            pressed_offset_y: match motion {
                MotionPolicy::Full => Some(px(1.0)),
                MotionPolicy::Reduced => None,
            },
        }
    }
}

type ActivationHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// One reusable native GPUI button.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    focus: FocusHandle,
    focus_visibility: FocusVisibility,
    theme: ArtisanTheme,
    motion: MotionPolicy,
    variant: ButtonVariant,
    size: ButtonSize,
    content: ButtonContent,
    disabled: bool,
    on_activate: Option<ActivationHandler>,
    debug_selector: Option<SharedString>,
}

impl Button {
    /// Constructs a button after validating the typed size/content pairing.
    ///
    /// # Errors
    ///
    /// Returns [`ButtonConstructionError`] when visible content has no usable
    /// label, or when square icon sizing and icon-only content are not selected
    /// together.
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        motion: MotionPolicy,
        variant: ButtonVariant,
        size: ButtonSize,
        content: ButtonContent,
    ) -> Result<Self, ButtonConstructionError> {
        if !content.is_icon_only() && content.accessible_label().trim().is_empty() {
            return Err(ButtonConstructionError::EmptyVisibleLabel);
        }

        match (size, content.is_icon_only()) {
            (ButtonSize::IconSmall, false) => {
                return Err(ButtonConstructionError::IconSizeRequiresIconOnly);
            }
            (ButtonSize::Small, true) => {
                return Err(ButtonConstructionError::IconOnlyRequiresIconSize);
            }
            (ButtonSize::IconSmall, true) | (ButtonSize::Small, false) => {}
        }

        Ok(Self {
            id: id.into(),
            focus,
            focus_visibility: FocusVisibility::Hidden,
            theme,
            motion,
            variant,
            size,
            content,
            disabled: false,
            on_activate: None,
            debug_selector: None,
        })
    }

    /// Selects whether actual focus should receive a visible ring.
    #[must_use]
    pub const fn focus_visibility(mut self, visibility: FocusVisibility) -> Self {
        self.focus_visibility = visibility;
        self
    }

    /// Selects the disabled presentation and suppresses every interaction.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Installs the activation callback used for primary pointer clicks and
    /// GPUI's unmodified Enter/Space keyboard clicks.
    #[must_use]
    pub fn on_activate(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Box::new(handler));
        self
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

    /// Returns the resolved theme/motion recipe.
    #[must_use]
    pub fn visual_style(&self) -> ButtonStyle {
        ButtonStyle::resolve(self.theme, self.variant, self.size, self.motion)
    }

    /// Whether this button should paint its keyboard-visible focus ring now.
    #[must_use]
    pub fn focus_ring_visible(&self, window: &Window) -> bool {
        !self.disabled
            && self.focus_visibility == FocusVisibility::Visible
            && self.focus.is_focused(window)
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _: &mut App) -> impl IntoElement {
        let style = self.visual_style();
        let disabled = self.disabled;
        let focus_visibility = self.focus_visibility;
        let focus = self.focus.clone();
        let on_activate = self.on_activate;

        let mut root = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .h(style.height)
            .px(style.horizontal_padding)
            .gap(style.content_gap)
            .rounded(style.corner_radius)
            .border_1()
            .border_color(style.border)
            .bg(style.background)
            .text_color(style.foreground)
            .text_size(self.theme.typography.control_text)
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when_some(style.width, gpui::Styled::w)
            .when(disabled, |element| element.opacity(style.disabled_opacity))
            .when_some(self.debug_selector, |element, selector| {
                element.debug_selector(move || selector.to_string())
            });

        root = match self.content {
            ButtonContent::Text(label) => root.child(label),
            ButtonContent::IconOnly { icon, .. } => {
                root.child(asset_glyph(icon).size(style.icon_size))
            }
            ButtonContent::IconText { icon, label } => root
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
                        }])
                })
            })
            .hover(move |hover| {
                hover
                    .bg(style.hover_background)
                    .text_color(style.hover_foreground)
            })
            .when_some(style.pressed_offset_y, |element, offset| {
                element.active(move |active| active.relative().top(offset))
            })
            .track_focus(&focus);

        if let Some(handler) = on_activate {
            root = root.on_click(move |event, window, cx| handler(event, window, cx));
        }

        root
    }
}
