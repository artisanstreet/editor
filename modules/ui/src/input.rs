//! Controlled native GPUI input surface for the reached text-field recipe.
//!
//! The legacy input is a browser `<input>` wrapper. Pinned GPUI 0.2.2 does
//! not provide an editable text element, a browser input type system, or a
//! platform accessibility tree, so this primitive deliberately stops at the
//! part GPUI can represent faithfully: a controlled value/placeholder
//! surface with focus, disabled, and invalid presentation. Callers retain
//! their text in [`crate::input_state::TextInputState`] and own the future
//! `InputHandler` composition. A file input is likewise a semantic marker;
//! callers that need a file chooser invoke GPUI's window APIs themselves.

use gpui::{
    App, BoxShadow, Div, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StyleRefinement, Styled, Window, div, point,
    px,
};

pub use crate::button::FocusVisibility;
use crate::input_state::TextInputState;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};

/// The stable selector used when a caller does not provide an instance name.
pub const DEFAULT_DEBUG_SELECTOR: &str = "artisan-input";

const BORDER_WIDTH_PX: f32 = 1.0;
const DESKTOP_LINE_HEIGHT_PX: f32 = 20.0;
const DISABLED_OPACITY: f32 = 0.5;

/// Browser input intents that have a reached native composition.
///
/// GPUI does not interpret these as browser attributes. [`InputType::File`]
/// only tells composition and tests that the caller intends a file-valued
/// control; it does not open a dialog or manufacture a `FileList`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum InputType {
    /// The default free-form text intent.
    #[default]
    Text,
    /// A caller-validated numeric text intent.
    Number,
    /// A caller-owned file-selection intent.
    File,
}

impl InputType {
    /// Whether this intent is the file branch of the legacy wrapper.
    #[must_use]
    pub const fn is_file(self) -> bool {
        matches!(self, Self::File)
    }

    /// Whether this intent is represented by the non-file visual branch.
    #[must_use]
    pub const fn is_non_file(self) -> bool {
        !self.is_file()
    }
}

/// Alias for callers whose domain names the field's kind rather than type.
pub type InputKind = InputType;

/// Theme-resolved geometry and paint for one input state.
///
/// The record is public so deterministic tests and future composition can
/// inspect the same recipe that the renderer applies. The `border`,
/// `focus_border`, and `focus_ring` fields already include the invalid-state
/// decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputStyle {
    /// Default control height: the legacy `h-9`, 36 px.
    pub height: Pixels,
    /// Horizontal control padding: the legacy `px-3`, 12 px.
    pub horizontal_padding: Pixels,
    /// Vertical control padding: the legacy `py-1`, 4 px.
    pub vertical_padding: Pixels,
    /// Full-pill radius: the legacy `rounded-4xl`, 26 px ramp step.
    pub corner_radius: Pixels,
    /// One-pixel input border.
    pub border_width: Pixels,
    /// Theme surface 100 in light mode and surface 900 in dark mode.
    pub background: Hsla,
    /// Normal value text color (`--foreground`).
    pub foreground: Hsla,
    /// Empty value text color (`--muted-foreground`).
    pub placeholder_foreground: Hsla,
    /// Current border, including the invalid-state branch.
    pub border: Hsla,
    /// Border color while the explicitly visible focus state is active.
    pub focus_border: Hsla,
    /// Ring color while the explicitly visible focus state is active.
    pub focus_ring: Hsla,
    /// Focus ring width: the legacy `[3px]` treatment.
    pub focus_ring_width: Pixels,
    /// Native desktop input text size (`md:text-sm`, 14 px).
    pub text_size: Pixels,
    /// One-line desktop input leading, 20 px.
    pub line_height: Pixels,
    /// Disabled presentation opacity from the legacy wrapper.
    pub disabled_opacity: f32,
    /// Whether this recipe was resolved for an invalid value.
    pub invalid: bool,
}

impl InputStyle {
    /// Resolves the reached input recipe from the shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, invalid: bool) -> Self {
        let surface = match theme.mode {
            ThemeMode::Light => SurfaceStep::S100,
            ThemeMode::Dark => SurfaceStep::S900,
        };

        let (border, focus_border, focus_ring) = if invalid {
            let border = match theme.mode {
                ThemeMode::Light => theme.colors.destructive.to_paint(),
                ThemeMode::Dark => theme.colors.destructive.with_alpha(0.5).to_paint(),
            };
            let ring = match theme.mode {
                ThemeMode::Light => theme.interaction.invalid_ring_color.to_paint(),
                ThemeMode::Dark => theme.colors.destructive.with_alpha(0.4).to_paint(),
            };
            (border, border, ring)
        } else {
            (
                theme.colors.input.to_paint(),
                theme.colors.ring.to_paint(),
                theme.interaction.focus_ring_color.to_paint(),
            )
        };

        Self {
            height: theme.density.control_default,
            horizontal_padding: theme.spacing.steps(3.0),
            vertical_padding: theme.spacing.steps(1.0),
            corner_radius: RadiusTokens::value(RadiusStep::X4l),
            border_width: px(BORDER_WIDTH_PX),
            background: theme.surfaces.value(surface).to_paint(),
            foreground: theme.colors.foreground.to_paint(),
            placeholder_foreground: theme.colors.muted_foreground.to_paint(),
            border,
            focus_border,
            focus_ring,
            focus_ring_width: theme.interaction.focus_ring_width,
            text_size: theme.typography.editor_text_desktop,
            line_height: px(DESKTOP_LINE_HEIGHT_PX),
            disabled_opacity: DISABLED_OPACITY,
            invalid,
        }
    }
}

impl From<ArtisanTheme> for InputStyle {
    fn from(theme: ArtisanTheme) -> Self {
        Self::resolve(theme, false)
    }
}

/// Compact flags for the caller-visible semantic state of one input.
///
/// GPUI 0.2.2 does not expose a platform accessibility tree. These flags
/// retain the deterministic state decisions needed by composition and tests
/// without presenting them as platform semantics.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct InputFlags(u8);

impl InputFlags {
    /// The input is disabled.
    pub const DISABLED: Self = Self(0b0000_0001);
    /// The input is invalid.
    pub const INVALID: Self = Self(0b0000_0010);
    /// The controlled value is non-empty.
    pub const HAS_VALUE: Self = Self(0b0000_0100);
    /// The placeholder branch is rendered.
    pub const SHOWS_PLACEHOLDER: Self = Self(0b0000_1000);
    /// The input participates in GPUI focus tracking.
    pub const FOCUSABLE: Self = Self(0b0001_0000);

    /// Returns these flags with `flag` enabled or disabled.
    #[must_use]
    pub const fn with(self, flag: Self, enabled: bool) -> Self {
        if enabled {
            Self(self.0 | flag.0)
        } else {
            Self(self.0 & !flag.0)
        }
    }

    /// Returns whether all bits in `flag` are set.
    #[must_use]
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    /// Returns whether the input is disabled.
    #[must_use]
    pub const fn is_disabled(self) -> bool {
        self.contains(Self::DISABLED)
    }

    /// Returns whether the input is invalid.
    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.contains(Self::INVALID)
    }

    /// Returns whether the controlled value is non-empty.
    #[must_use]
    pub const fn has_value(self) -> bool {
        self.contains(Self::HAS_VALUE)
    }

    /// Returns whether the placeholder branch is rendered.
    #[must_use]
    pub const fn shows_placeholder(self) -> bool {
        self.contains(Self::SHOWS_PLACEHOLDER)
    }

    /// Returns whether the input participates in GPUI focus tracking.
    #[must_use]
    pub const fn is_focusable(self) -> bool {
        self.contains(Self::FOCUSABLE)
    }
}

/// Caller-visible semantic state retained without claiming platform
/// accessibility support that GPUI 0.2.2 does not expose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputSemantics {
    /// The caller's requested input intent.
    pub input_type: InputType,
    /// Compact disabled, invalid, value, placeholder, and focusability flags.
    pub flags: InputFlags,
    /// Optional caller-provided label metadata for future accessibility
    /// wiring; this value is not registered with a platform accessibility API.
    pub semantic_label: Option<SharedString>,
}

/// A controlled, focusable native input surface.
///
/// The value and placeholder are rendered verbatim. This component does not
/// mutate text, expose browser `type` behavior, synthesize a `FileList`, or
/// claim an accessibility role. Use [`TextInputState`] for controlled text
/// state and compose an `InputHandler` around the surface when editable GPUI
/// content is available to the caller.
#[derive(IntoElement)]
pub struct Input {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    value: SharedString,
    placeholder: Option<SharedString>,
    kind: InputType,
    disabled: bool,
    invalid: bool,
    focus_visibility: FocusVisibility,
    semantic_label: Option<SharedString>,
    debug_selector: Option<SharedString>,
    root: Div,
}

impl Input {
    /// Constructs a text input from a controlled value.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        value: impl Into<SharedString>,
    ) -> Self {
        let style = InputStyle::resolve(theme, false);
        Self {
            id: id.into(),
            focus,
            theme,
            value: value.into(),
            placeholder: None,
            kind: InputType::Text,
            disabled: false,
            invalid: false,
            focus_visibility: FocusVisibility::Hidden,
            semantic_label: None,
            debug_selector: Some(DEFAULT_DEBUG_SELECTOR.into()),
            root: input_root(style),
        }
    }

    /// Constructs an input from the repository's canonical controlled text
    /// state without taking ownership of or duplicating that state object.
    #[must_use]
    pub fn from_state(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        state: &TextInputState,
    ) -> Self {
        Self::new(id, focus, theme, state.value().to_owned())
    }

    /// Changes the caller's semantic input intent.
    #[must_use]
    pub const fn input_type(mut self, input_type: InputType) -> Self {
        self.kind = input_type;
        self
    }

    /// Alias for [`Self::input_type`].
    #[must_use]
    pub const fn kind(self, kind: InputKind) -> Self {
        self.input_type(kind)
    }

    /// Supplies the controlled placeholder text.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Removes the controlled placeholder branch.
    #[must_use]
    pub fn without_placeholder(mut self) -> Self {
        self.placeholder = None;
        self
    }

    /// Sets the invalid visual and semantic state.
    #[must_use]
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        let border = self.visual_style().border;
        self.root = self.root.border_color(border);
        self
    }

    /// Sets the disabled visual and semantic state.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Supplies the caller's explicit focus-ring visibility decision.
    #[must_use]
    pub const fn focus_visibility(mut self, focus_visibility: FocusVisibility) -> Self {
        self.focus_visibility = focus_visibility;
        self
    }

    /// Retains a semantic label for tests and future accessibility wiring.
    ///
    /// GPUI 0.2.2 does not publish this value to a platform accessibility
    /// tree. It is intentionally metadata only.
    #[must_use]
    pub fn semantic_label(mut self, label: impl Into<SharedString>) -> Self {
        self.semantic_label = Some(label.into());
        self
    }

    /// Alias for [`Self::semantic_label`].
    #[must_use]
    pub fn accessible_label(self, label: impl Into<SharedString>) -> Self {
        self.semantic_label(label)
    }

    /// Adds a stable GPUI debug selector to the root and value branch.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the theme-resolved visual recipe for the current invalid state.
    #[must_use]
    pub fn visual_style(&self) -> InputStyle {
        InputStyle::resolve(self.theme, self.invalid)
    }

    /// Returns the controlled value verbatim.
    #[must_use]
    pub fn value(&self) -> &str {
        self.value.as_str()
    }

    /// Returns the controlled placeholder, if one was supplied.
    #[must_use]
    pub fn placeholder_value(&self) -> Option<&str> {
        self.placeholder.as_ref().map(SharedString::as_str)
    }

    /// Returns the requested input intent.
    #[must_use]
    pub const fn input_type_value(&self) -> InputType {
        self.kind
    }

    /// Returns whether this is the caller-owned file branch.
    #[must_use]
    pub const fn is_file(&self) -> bool {
        self.kind.is_file()
    }

    /// Returns whether this input is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns whether this input is invalid.
    #[must_use]
    pub const fn is_invalid(&self) -> bool {
        self.invalid
    }

    /// Returns the current semantic decisions for caller composition/tests.
    #[must_use]
    pub fn semantics(&self) -> InputSemantics {
        let has_value = !self.value.as_str().is_empty();
        let shows_placeholder = !has_value && self.placeholder.is_some();
        InputSemantics {
            input_type: self.kind,
            flags: InputFlags::default()
                .with(InputFlags::DISABLED, self.disabled)
                .with(InputFlags::INVALID, self.invalid)
                .with(InputFlags::HAS_VALUE, has_value)
                .with(InputFlags::SHOWS_PLACEHOLDER, shows_placeholder)
                .with(InputFlags::FOCUSABLE, !self.disabled),
            semantic_label: self.semantic_label.clone(),
        }
    }

    /// Whether the actual focus handle and explicit visibility decision permit
    /// the focus ring to paint in this window.
    #[must_use]
    pub fn focus_ring_visible(&self, window: &Window) -> bool {
        !self.disabled
            && self.focus_visibility == FocusVisibility::Visible
            && self.focus.is_focused(window)
    }
}

impl Styled for Input {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

impl RenderOnce for Input {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let style = self.visual_style();

        let Self {
            id,
            focus,
            value,
            placeholder,
            disabled,
            focus_visibility,
            debug_selector,
            mut root,
            ..
        } = self;

        let has_value = !value.as_str().is_empty();
        let shows_placeholder = !has_value && placeholder.is_some();
        let text = if has_value {
            value
        } else {
            placeholder.unwrap_or_default()
        };

        let branch_name = if has_value {
            "value"
        } else if shows_placeholder {
            "placeholder"
        } else {
            "empty"
        };

        let mut content = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .overflow_hidden()
            .whitespace_nowrap()
            .child(text);

        if shows_placeholder {
            content = content.text_color(style.placeholder_foreground);
        }

        let root_selector = debug_selector.map_or_else(
            || DEFAULT_DEBUG_SELECTOR.to_owned(),
            |selector| selector.to_string(),
        );
        let branch_selector = format!("{root_selector}-{branch_name}");

        root = root.debug_selector(move || root_selector.clone());
        content = content.debug_selector(move || branch_selector.clone());

        if disabled {
            root = root.opacity(style.disabled_opacity);
        }

        let mut root = root.id(id);
        if disabled {
            return root.child(content);
        }

        if focus_visibility == FocusVisibility::Visible {
            root = root.focus(move |focused| {
                focused
                    .border_color(style.focus_border)
                    .shadow(vec![BoxShadow {
                        color: style.focus_ring,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: style.focus_ring_width,
                    }])
            });
        }

        root.track_focus(&focus).child(content)
    }
}

fn input_root(style: InputStyle) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .h(style.height)
        .px(style.horizontal_padding)
        .py(style.vertical_padding)
        .rounded(style.corner_radius)
        .border(style.border_width)
        .border_color(style.border)
        .bg(style.background)
        .text_color(style.foreground)
        .text_size(style.text_size)
        .line_height(style.line_height)
        .whitespace_nowrap()
        .overflow_hidden()
}
