//! Compact native GPUI avatar primitive with caller-controlled image state.
//!
//! The reached legacy avatar is a 32 px circular root with a muted fallback
//! (`bg-muted text-muted-foreground text-xs font-medium`) and an optional
//! square image child. Image loading remains outside this component: callers
//! provide both the image element and its explicit [`AvatarImageState`]. Only
//! [`AvatarImageState::Loaded`] admits that child; every other state, including
//! an absent child, presents the fallback text.

use gpui::{
    AnyElement, App, Div, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Pixels,
    RenderOnce, SharedString, StyleRefinement, Styled, Window, div, px,
};

use crate::theme::ArtisanTheme;

/// The stable selector used when a caller does not provide an instance name.
pub const DEFAULT_DEBUG_SELECTOR: &str = "artisan-avatar";

const DEFAULT_DIAMETER_PX: f32 = 32.0;
const FULL_RADIUS_PX: f32 = 9999.0;
const FALLBACK_LINE_HEIGHT_PX: f32 = 16.0;

/// The caller-owned image lifecycle decision.
///
/// The avatar does not load, retry, or infer this state. A loaded state still
/// needs an image child; without one the component safely presents its
/// fallback.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AvatarImageState {
    /// The image has not settled and must not be painted yet.
    #[default]
    Pending,
    /// The supplied image child is eligible to be rendered.
    Loaded,
    /// Loading settled unsuccessfully and the fallback remains visible.
    Failed,
}

impl AvatarImageState {
    /// Whether this state permits an existing image child to render.
    #[must_use]
    pub const fn renders_image(self) -> bool {
        matches!(self, Self::Loaded)
    }
}

/// Short alias for callers that already use a generic image-state vocabulary.
pub type ImageState = AvatarImageState;

/// Theme-resolved geometry and fallback paint for one avatar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AvatarStyle {
    /// Default root diameter, matching the legacy `size-8` recipe.
    pub diameter: Pixels,
    /// The intentionally oversized radius used by `rounded-full`.
    pub corner_radius: Pixels,
    /// Muted fallback surface (`--muted`).
    pub background: Hsla,
    /// Muted fallback text (`--muted-foreground`).
    pub foreground: Hsla,
    /// Fallback label size (`text-xs`).
    pub text_size: Pixels,
    /// Fallback label line height (`text-xs`'s 1 rem leading).
    pub line_height: Pixels,
}

impl AvatarStyle {
    /// Resolves the reached avatar recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            diameter: px(DEFAULT_DIAMETER_PX),
            corner_radius: px(FULL_RADIUS_PX),
            background: theme.colors.muted.to_paint(),
            foreground: theme.colors.muted_foreground.to_paint(),
            text_size: theme.typography.label_text,
            line_height: px(FALLBACK_LINE_HEIGHT_PX),
        }
    }
}

impl From<ArtisanTheme> for AvatarStyle {
    fn from(theme: ArtisanTheme) -> Self {
        Self::resolve(theme)
    }
}

/// Constructs a compact avatar from a theme or an already resolved style.
#[must_use]
pub fn avatar(style: impl Into<AvatarStyle>, fallback: impl Into<SharedString>) -> Avatar {
    Avatar::new(style, fallback)
}

/// A compact, noninteractive avatar whose image presentation is controlled by
/// the caller.
///
/// [`Styled`] methods refine the root element held by this component. Because
/// the defaults are installed during construction and not reapplied during
/// [`RenderOnce::render`], later caller values win as they do on a plain GPUI
/// `Div`.
#[derive(IntoElement)]
pub struct Avatar {
    style: AvatarStyle,
    root: Div,
    fallback: SharedString,
    image: Option<AnyElement>,
    image_state: AvatarImageState,
    debug_selector: Option<SharedString>,
}

impl Avatar {
    /// Constructs an avatar from either a theme or an already resolved style.
    ///
    /// The fallback is caller-supplied text and is kept verbatim; this
    /// component does not derive initials or otherwise transform it.
    #[must_use]
    pub fn new(style: impl Into<AvatarStyle>, fallback: impl Into<SharedString>) -> Self {
        let style = style.into();
        let root = div()
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .w(style.diameter)
            .h(style.diameter)
            .rounded(style.corner_radius)
            .overflow_hidden()
            .bg(style.background)
            .text_color(style.foreground)
            .text_size(style.text_size)
            .font_weight(FontWeight::MEDIUM)
            .line_height(style.line_height)
            .whitespace_nowrap();

        Self {
            style,
            root,
            fallback: fallback.into(),
            image: None,
            image_state: AvatarImageState::Pending,
            debug_selector: Some(DEFAULT_DEBUG_SELECTOR.into()),
        }
    }

    /// Constructs an avatar with the supplied image and lifecycle state.
    #[must_use]
    pub fn new_with_image(
        style: impl Into<AvatarStyle>,
        fallback: impl Into<SharedString>,
        image: impl IntoElement,
        image_state: AvatarImageState,
    ) -> Self {
        Self::new(style, fallback).image(image, image_state)
    }

    /// Returns the resolved recipe retained by this component.
    #[must_use]
    pub const fn visual_style(&self) -> AvatarStyle {
        self.style
    }

    /// Supplies an image child and its caller-owned lifecycle state.
    ///
    /// The child is retained without loading or inspecting it. It is inserted
    /// only when `image_state` is [`AvatarImageState::Loaded`].
    #[must_use]
    pub fn image(mut self, image: impl IntoElement, image_state: AvatarImageState) -> Self {
        self.image = Some(image.into_any_element());
        self.image_state = image_state;
        self
    }

    /// Alias for [`Self::image`] that reads naturally in configuration code.
    #[must_use]
    pub fn with_image(self, image: impl IntoElement, image_state: AvatarImageState) -> Self {
        self.image(image, image_state)
    }

    /// Changes the explicit state while retaining the current image child.
    #[must_use]
    pub const fn image_state(mut self, image_state: AvatarImageState) -> Self {
        self.image_state = image_state;
        self
    }

    /// Adds a stable GPUI debug selector to the root and its branch child.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the explicit state retained by this component.
    #[must_use]
    pub const fn image_state_value(&self) -> AvatarImageState {
        self.image_state
    }
}

impl Styled for Avatar {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

impl RenderOnce for Avatar {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            mut root,
            fallback,
            image,
            image_state,
            debug_selector,
            ..
        } = self;

        let branch_selector = debug_selector
            .as_ref()
            .map(std::string::ToString::to_string);

        if let Some(selector) = debug_selector {
            root = root.debug_selector(move || selector.to_string());
        }

        let branch = if image_state.renders_image() {
            if let Some(image) = image {
                let mut image_container = div().size_full().flex_shrink_0().child(image);
                if let Some(selector) = branch_selector.as_ref() {
                    let selector = format!("{selector}-image");
                    image_container = image_container.debug_selector(move || selector.clone());
                }
                image_container
            } else {
                fallback_element(fallback, branch_selector)
            }
        } else {
            fallback_element(fallback, branch_selector)
        };

        root.child(branch)
    }
}

fn fallback_element(fallback: SharedString, selector: Option<String>) -> Div {
    let mut element = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .size_full()
        .flex_shrink_0()
        .child(fallback);

    if let Some(selector) = selector {
        let selector = format!("{selector}-fallback");
        element = element.debug_selector(move || selector.clone());
    }

    element
}
