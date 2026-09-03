//! Controlled native GPUI textarea surface for the reached form recipe.
//!
//! The legacy textarea is a browser `<textarea>` wrapper. Pinned GPUI 0.2.2
//! does not provide an editable multiline widget, a browser textarea type
//! system, or a platform accessibility tree, so this primitive deliberately
//! stops at the part GPUI can represent faithfully: a controlled
//! value/placeholder surface with multiline/newline-aware state, focus,
//! disabled, and invalid presentation. Callers retain their text in
//! [`crate::input_state::TextInputState`] and own the future multiline
//! `InputHandler` composition. Value newlines are preserved verbatim and
//! rendered as wrapped line breaks; no trimming, collapsing, or synthetic
//! HTML/DOM behavior is introduced.

use gpui::{
    App, BoxShadow, Div, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StyleRefinement, Styled, Window, div, point,
    px,
};

pub use crate::button::FocusVisibility;
use crate::input_state::TextInputState;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};

/// The stable selector used when a caller does not provide an instance name.
pub const DEFAULT_DEBUG_SELECTOR: &str = "artisan-textarea";

const BORDER_WIDTH_PX: f32 = 1.0;
const DISABLED_OPACITY: f32 = 0.5;

/// Returns the number of logical lines in `value`.
///
/// Newlines are the only line separator, matching [`crate::input_state::normalize_input`]
/// which canonicalizes CRLF and CR to LF on intake. An empty value has zero
/// lines; otherwise the result is `newline_count + 1`.
///
/// # Examples
///
/// ```
/// # use artisan_ui::textarea::textarea_line_count;
/// assert_eq!(textarea_line_count(""), 0);
/// assert_eq!(textarea_line_count("hello"), 1);
/// assert_eq!(textarea_line_count("a\nb\n"), 3);
/// ```
#[must_use]
pub fn textarea_line_count(value: &str) -> usize {
    if value.is_empty() {
        0
    } else {
        value.chars().filter(|character| *character == '\n').count() + 1
    }
}

/// Returns whether `value` contains at least one newline.
#[must_use]
pub fn textarea_has_newline(value: &str) -> bool {
    value.contains('\n')
}

/// Theme-resolved geometry and paint for one textarea state.
///
/// The record is public so deterministic tests and future composition can
/// inspect the same recipe that the renderer applies. The `border`,
/// `focus_border`, and `focus_ring` fields already include the invalid-state
/// decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextareaStyle {
    /// Minimum control height: the legacy `min-h-16`, 64 px.
    pub min_height: Pixels,
    /// Horizontal control padding: the legacy `px-3`, 12 px.
    pub horizontal_padding: Pixels,
    /// Vertical control padding: the legacy `py-3`, 12 px.
    pub vertical_padding: Pixels,
    /// Rounded corner: the legacy `rounded-xl`, 14 px ramp step.
    pub corner_radius: Pixels,
    /// One-pixel textarea border.
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
    /// Native desktop textarea text size (`md:text-sm`, 14 px).
    pub text_size: Pixels,
    /// Desktop textarea leading, 20 px.
    pub line_height: Pixels,
    /// Disabled presentation opacity from the legacy wrapper.
    pub disabled_opacity: f32,
    /// Whether this recipe was resolved for an invalid value.
    pub invalid: bool,
}

impl TextareaStyle {
    /// Resolves the reached textarea recipe from the shared theme tokens.
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
            min_height: theme.spacing.steps(16.0),
            horizontal_padding: theme.spacing.steps(3.0),
            vertical_padding: theme.spacing.steps(3.0),
            corner_radius: RadiusTokens::value(RadiusStep::Xl),
            border_width: px(BORDER_WIDTH_PX),
            background: theme.surfaces.value(surface).to_paint(),
            foreground: theme.colors.foreground.to_paint(),
            placeholder_foreground: theme.colors.muted_foreground.to_paint(),
            border,
            focus_border,
            focus_ring,
            focus_ring_width: theme.interaction.focus_ring_width,
            text_size: theme.typography.editor_text_desktop,
            line_height: px(20.0),
            disabled_opacity: DISABLED_OPACITY,
            invalid,
        }
    }
}

impl From<ArtisanTheme> for TextareaStyle {
    fn from(theme: ArtisanTheme) -> Self {
        Self::resolve(theme, false)
    }
}

/// Compact flags for the caller-visible semantic state of one textarea.
///
/// GPUI 0.2.2 does not expose a platform accessibility tree. These flags
/// retain the deterministic state decisions needed by composition and tests
/// without presenting them as platform semantics.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TextareaFlags(u8);

impl TextareaFlags {
    /// The textarea is disabled.
    pub const DISABLED: Self = Self(0b0000_0001);
    /// The textarea is invalid.
    pub const INVALID: Self = Self(0b0000_0010);
    /// The controlled value is non-empty.
    pub const HAS_VALUE: Self = Self(0b0000_0100);
    /// The placeholder branch is rendered.
    pub const SHOWS_PLACEHOLDER: Self = Self(0b0000_1000);
    /// The textarea participates in GPUI focus tracking.
    pub const FOCUSABLE: Self = Self(0b0001_0000);
    /// The controlled value contains at least one newline.
    pub const HAS_NEWLINE: Self = Self(0b0010_0000);

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

    /// Returns whether the textarea is disabled.
    #[must_use]
    pub const fn is_disabled(self) -> bool {
        self.contains(Self::DISABLED)
    }

    /// Returns whether the textarea is invalid.
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

    /// Returns whether the textarea participates in GPUI focus tracking.
    #[must_use]
    pub const fn is_focusable(self) -> bool {
        self.contains(Self::FOCUSABLE)
    }

    /// Returns whether the controlled value contains a newline.
    #[must_use]
    pub const fn has_newline(self) -> bool {
        self.contains(Self::HAS_NEWLINE)
    }
}

/// Caller-visible semantic state retained without claiming platform
/// accessibility support that GPUI 0.2.2 does not expose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextareaSemantics {
    /// Compact disabled, invalid, value, placeholder, focusability, and newline flags.
    pub flags: TextareaFlags,
    /// Logical line count of the controlled value.
    pub line_count: usize,
    /// Optional caller-provided label metadata for future accessibility
    /// wiring; this value is not registered with a platform accessibility API.
    pub semantic_label: Option<SharedString>,
}

/// A controlled, focusable native textarea surface.
///
/// The value and placeholder are rendered verbatim, including any embedded
/// newlines. This component does not mutate text, handle keyboard input, or
/// claim an accessibility role. Use [`TextInputState`] for controlled text
/// state and compose an `InputHandler` around the surface when editable GPUI
/// content is available to the caller.
#[derive(IntoElement)]
pub struct Textarea {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    value: SharedString,
    placeholder: Option<SharedString>,
    disabled: bool,
    invalid: bool,
    focus_visibility: FocusVisibility,
    semantic_label: Option<SharedString>,
    debug_selector: Option<SharedString>,
    root: Div,
}

impl Textarea {
    /// Constructs a textarea from a controlled value.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        value: impl Into<SharedString>,
    ) -> Self {
        let style = TextareaStyle::resolve(theme, false);
        Self {
            id: id.into(),
            focus,
            theme,
            value: value.into(),
            placeholder: None,
            disabled: false,
            invalid: false,
            focus_visibility: FocusVisibility::Hidden,
            semantic_label: None,
            debug_selector: Some(DEFAULT_DEBUG_SELECTOR.into()),
            root: textarea_root(style),
        }
    }

    /// Constructs a textarea from the repository's canonical controlled text
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
    pub fn visual_style(&self) -> TextareaStyle {
        TextareaStyle::resolve(self.theme, self.invalid)
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

    /// Returns the logical line count of the controlled value.
    #[must_use]
    pub fn line_count(&self) -> usize {
        textarea_line_count(self.value.as_str())
    }

    /// Returns whether the controlled value contains a newline.
    #[must_use]
    pub fn has_newline(&self) -> bool {
        textarea_has_newline(self.value.as_str())
    }

    /// Returns whether this textarea is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns whether this textarea is invalid.
    #[must_use]
    pub const fn is_invalid(&self) -> bool {
        self.invalid
    }

    /// Returns the current semantic decisions for caller composition/tests.
    #[must_use]
    pub fn semantics(&self) -> TextareaSemantics {
        let has_value = !self.value.as_str().is_empty();
        let shows_placeholder = !has_value && self.placeholder.is_some();
        let has_newline = textarea_has_newline(self.value.as_str());
        TextareaSemantics {
            flags: TextareaFlags::default()
                .with(TextareaFlags::DISABLED, self.disabled)
                .with(TextareaFlags::INVALID, self.invalid)
                .with(TextareaFlags::HAS_VALUE, has_value)
                .with(TextareaFlags::SHOWS_PLACEHOLDER, shows_placeholder)
                .with(TextareaFlags::FOCUSABLE, !self.disabled)
                .with(TextareaFlags::HAS_NEWLINE, has_newline),
            line_count: textarea_line_count(self.value.as_str()),
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

impl Styled for Textarea {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

impl RenderOnce for Textarea {
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
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .w_full()
            .overflow_hidden()
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

fn textarea_root(style: TextareaStyle) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w(px(0.0))
        .min_h(style.min_height)
        .px(style.horizontal_padding)
        .py(style.vertical_padding)
        .rounded(style.corner_radius)
        .border(style.border_width)
        .border_color(style.border)
        .bg(style.background)
        .text_color(style.foreground)
        .text_size(style.text_size)
        .line_height(style.line_height)
        .overflow_hidden()
}
