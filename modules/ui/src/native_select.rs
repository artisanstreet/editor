//! Compact native GPUI option selector for the reached form surfaces.
//!
//! The legacy frontend renders a browser `<select>` with a chevron overlay
//! (`modules/frontend/src/lib/components/ui/native-select/native-select.svelte`).
//! Pinned GPUI 0.2.2 has no native select element, no browser input type
//! system, and no platform accessibility tree, so this primitive stops at the
//! part GPUI can represent faithfully: a controlled value paired with an
//! explicit option list, caller-owned disabled/invalid/placeholder semantics,
//! deterministic option lookup, and theme-aware control/chevron rendering. The
//! value is always caller-owned; an interaction never mutates retained state
//! and only produces a change intent through [`NativeSelect::next_value_for`]
//! or the installed [`NativeSelect::on_change`] callback.
//!
//! Deliberately absent because no reached call site uses them: label/value
//! search, typeahead, multi-select, grouped options, async option loading,
//! form submission, and the custom popover `Select` already owned separately
//! (`select.rs`). That component stays under separate ownership and is never
//! duplicated here.

use gpui::{
    App, BoxShadow, Div, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, StatefulInteractiveElement, StyleRefinement,
    Styled, Window, div, point, px,
};

use crate::asset_seam::asset_glyph;
use crate::button::FocusVisibility;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};

/// The stable selector used when a caller does not provide an instance name.
pub const DEFAULT_DEBUG_SELECTOR: &str = "artisan-native-select";

const BORDER_WIDTH_PX: f32 = 1.0;
const CHEVRON_SIZE_PX: f32 = 16.0;
const DISABLED_OPACITY: f32 = 0.5;
const DESKTOP_LINE_HEIGHT_PX: f32 = 20.0;

/// The reached native-select control sizes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum NativeSelectSize {
    /// Default `h-9`, 36 px, matching `native-select.svelte` default.
    #[default]
    Default,
    /// Compact `h-8`, 32 px, matching `data-[size=sm]:h-8`.
    Small,
}

/// One explicit option in a native select.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSelectOption {
    value: SharedString,
    label: SharedString,
    disabled: bool,
}

impl NativeSelectOption {
    /// Creates an option from a stable value and visible label.
    ///
    /// # Errors
    ///
    /// Returns [`NativeSelectConstructionError`] when `value` or `label` is
    /// empty or contains only whitespace.
    pub fn new(
        value: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> Result<Self, NativeSelectConstructionError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(NativeSelectConstructionError::EmptyOptionValue);
        }
        let label = label.into();
        if label.trim().is_empty() {
            return Err(NativeSelectConstructionError::EmptyOptionLabel);
        }
        Ok(Self {
            value,
            label,
            disabled: false,
        })
    }

    /// Marks this option disabled, preventing its selection via change intent.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the stable option value.
    #[must_use]
    pub fn value(&self) -> &str {
        self.value.as_str()
    }

    /// Returns the visible option label.
    #[must_use]
    pub fn label(&self) -> &str {
        self.label.as_str()
    }

    /// Returns whether this option is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// Construction failure for a native select or its options.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NativeSelectConstructionError {
    /// An option value was empty or whitespace-only.
    #[error("native-select option value cannot be empty or whitespace-only")]
    EmptyOptionValue,
    /// An option label was empty or whitespace-only.
    #[error("native-select option label cannot be empty or whitespace-only")]
    EmptyOptionLabel,
    /// Two options share the same stable value.
    #[error("native-select option values must be unique")]
    DuplicateOptionValue,
    /// The option list is empty.
    #[error("native-select requires at least one option")]
    EmptyOptions,
}

/// Theme-resolved geometry and paint for one native-select state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NativeSelectStyle {
    /// Control height: 36 px default or 32 px small.
    pub height: Pixels,
    /// Left padding: the legacy `pl-3`, 12 px.
    pub padding_left: Pixels,
    /// Right padding: the legacy `pr-8`, 32 px, reserving space for the chevron.
    pub padding_right: Pixels,
    /// Vertical padding: the legacy `py-1`, 4 px.
    pub padding_vertical: Pixels,
    /// Full-pill radius: the legacy `rounded-4xl`, 26 px.
    pub corner_radius: Pixels,
    /// One-pixel control border.
    pub border_width: Pixels,
    /// Theme background (`input/30` at 30% alpha).
    pub background: Hsla,
    /// Normal value text color.
    pub foreground: Hsla,
    /// Placeholder/empty text color (`--muted-foreground`).
    pub placeholder_foreground: Hsla,
    /// Current border color, including invalid state.
    pub border: Hsla,
    /// Border while the explicit focus state is visible.
    pub focus_border: Hsla,
    /// Ring while the explicit focus state is visible.
    pub focus_ring: Hsla,
    /// Focus ring width, 3 px.
    pub focus_ring_width: Pixels,
    /// Chevron edge, 16 px.
    pub chevron_size: Pixels,
    /// Chevron paint (`--muted-foreground`).
    pub chevron_color: Hsla,
    /// Native desktop text size, 14 px.
    pub text_size: Pixels,
    /// Desktop line height, 20 px.
    pub line_height: Pixels,
    /// Disabled opacity, 0.5.
    pub disabled_opacity: f32,
    /// Whether this recipe was resolved for an invalid value.
    pub invalid: bool,
}

impl NativeSelectStyle {
    /// Resolves the reached native-select recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, size: NativeSelectSize, invalid: bool) -> Self {
        let height = match size {
            NativeSelectSize::Default => theme.density.control_default,
            NativeSelectSize::Small => theme.density.control_sm,
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
            height,
            padding_left: theme.spacing.steps(3.0),
            padding_right: theme.spacing.steps(8.0),
            padding_vertical: theme.spacing.steps(1.0),
            corner_radius: RadiusTokens::value(RadiusStep::X4l),
            border_width: px(BORDER_WIDTH_PX),
            background: theme.colors.input.with_alpha(0.3).to_paint(),
            foreground: theme.colors.foreground.to_paint(),
            placeholder_foreground: theme.colors.muted_foreground.to_paint(),
            border,
            focus_border,
            focus_ring,
            focus_ring_width: theme.interaction.focus_ring_width,
            chevron_size: px(CHEVRON_SIZE_PX),
            chevron_color: theme.colors.muted_foreground.to_paint(),
            text_size: theme.typography.editor_text_desktop,
            line_height: px(DESKTOP_LINE_HEIGHT_PX),
            disabled_opacity: DISABLED_OPACITY,
            invalid,
        }
    }
}

/// Compact flags for the caller-visible semantic state of one native select.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct NativeSelectFlags(u8);

impl NativeSelectFlags {
    /// The select is disabled.
    pub const DISABLED: Self = Self(0b0000_0001);
    /// The select is invalid.
    pub const INVALID: Self = Self(0b0000_0010);
    /// The controlled value matches an option.
    pub const HAS_VALUE: Self = Self(0b0000_0100);
    /// The placeholder branch is rendered.
    pub const SHOWS_PLACEHOLDER: Self = Self(0b0000_1000);
    /// The select participates in GPUI focus tracking.
    pub const FOCUSABLE: Self = Self(0b0001_0000);
    /// The controlled value matches a disabled option.
    pub const HAS_DISABLED_SELECTION: Self = Self(0b0010_0000);

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

    /// Returns whether the select is disabled.
    #[must_use]
    pub const fn is_disabled(self) -> bool {
        self.contains(Self::DISABLED)
    }

    /// Returns whether the select is invalid.
    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.contains(Self::INVALID)
    }

    /// Returns whether the controlled value matches an option.
    #[must_use]
    pub const fn has_value(self) -> bool {
        self.contains(Self::HAS_VALUE)
    }

    /// Returns whether the placeholder branch is rendered.
    #[must_use]
    pub const fn shows_placeholder(self) -> bool {
        self.contains(Self::SHOWS_PLACEHOLDER)
    }

    /// Returns whether the select participates in GPUI focus tracking.
    #[must_use]
    pub const fn is_focusable(self) -> bool {
        self.contains(Self::FOCUSABLE)
    }

    /// Returns whether the current value matches a disabled option.
    #[must_use]
    pub const fn has_disabled_selection(self) -> bool {
        self.contains(Self::HAS_DISABLED_SELECTION)
    }
}

/// Caller-visible semantic state retained without claiming platform
/// accessibility support that GPUI 0.2.2 does not expose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSelectSemantics {
    /// Compact disabled, invalid, value, placeholder, and focusability flags.
    pub flags: NativeSelectFlags,
    /// The controlled value verbatim.
    pub value: SharedString,
    /// The label of the matching option, if any.
    pub selected_label: Option<SharedString>,
    /// The placeholder, if one was supplied.
    pub placeholder: Option<SharedString>,
    /// Number of options.
    pub option_count: usize,
    /// Whether the selected option is disabled, if a selection exists.
    pub selected_is_disabled: bool,
    /// Optional caller-provided semantic label metadata for future wiring.
    pub semantic_label: Option<SharedString>,
    /// The rendered size.
    pub size: NativeSelectSize,
}

/// One explicit change intent emitted by a native select.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeSelectChange {
    /// The stable value to select next.
    pub value: SharedString,
    /// The visible label for that value.
    pub label: SharedString,
}

type ChangeHandler = Box<dyn Fn(NativeSelectChange, &mut Window, &mut App)>;

/// A controlled, focusable compact native option selector.
#[derive(IntoElement)]
pub struct NativeSelect {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    value: SharedString,
    options: Vec<NativeSelectOption>,
    placeholder: Option<SharedString>,
    disabled: bool,
    invalid: bool,
    size: NativeSelectSize,
    focus_visibility: FocusVisibility,
    semantic_label: Option<SharedString>,
    debug_selector: Option<SharedString>,
    on_change: Option<ChangeHandler>,
    root: Div,
}

impl NativeSelect {
    /// Constructs a native select from a controlled value and option list.
    ///
    /// The controlled value may be empty or absent from the option list; that
    /// is the empty/placeholder state, not a construction failure. Options
    /// themselves must have non-empty values and labels and unique values.
    ///
    /// # Errors
    ///
    /// Returns [`NativeSelectConstructionError`] when `options` is empty, any
    /// option carries an empty value or label (via [`NativeSelectOption::new`]),
    /// or two options share the same value.
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        value: impl Into<SharedString>,
        options: Vec<NativeSelectOption>,
    ) -> Result<Self, NativeSelectConstructionError> {
        if options.is_empty() {
            return Err(NativeSelectConstructionError::EmptyOptions);
        }
        let mut seen = std::collections::HashSet::new();
        for option in &options {
            if !seen.insert(option.value.as_str().to_owned()) {
                return Err(NativeSelectConstructionError::DuplicateOptionValue);
            }
        }
        let value = value.into();
        let style = NativeSelectStyle::resolve(theme, NativeSelectSize::Default, false);
        Ok(Self {
            id: id.into(),
            focus,
            theme,
            value,
            options,
            placeholder: None,
            disabled: false,
            invalid: false,
            size: NativeSelectSize::Default,
            focus_visibility: FocusVisibility::Hidden,
            semantic_label: None,
            debug_selector: Some(DEFAULT_DEBUG_SELECTOR.into()),
            on_change: None,
            root: native_select_root(style),
        })
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

    /// Sets the control size.
    #[must_use]
    pub fn size(mut self, size: NativeSelectSize) -> Self {
        self.size = size;
        let style = self.visual_style();
        self.root = native_select_root(style);
        if self.invalid {
            self.root = self.root.border_color(style.border);
        }
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

    /// Installs the controlled change callback.
    ///
    /// The callback receives the next stable value and label for the
    /// caller-owned controlled value. It is invoked only for enabled options
    /// whose value differs from the current controlled value.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(NativeSelectChange, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }

    /// Returns the theme-resolved visual recipe for the current state.
    #[must_use]
    pub fn visual_style(&self) -> NativeSelectStyle {
        NativeSelectStyle::resolve(self.theme, self.size, self.invalid)
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

    /// Returns the current options.
    #[must_use]
    pub fn options(&self) -> &[NativeSelectOption] {
        &self.options
    }

    /// Returns whether this select is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns whether this select is invalid.
    #[must_use]
    pub const fn is_invalid(&self) -> bool {
        self.invalid
    }

    /// Returns the rendered size.
    #[must_use]
    pub const fn size_value(&self) -> NativeSelectSize {
        self.size
    }

    /// Returns the matching option for the current controlled value, if any.
    #[must_use]
    pub fn selected_option(&self) -> Option<&NativeSelectOption> {
        self.options
            .iter()
            .find(|option| option.value.as_str() == self.value.as_str())
    }

    /// Returns the label for the current controlled value, if it matches an option.
    #[must_use]
    pub fn selected_label(&self) -> Option<&str> {
        self.selected_option().map(NativeSelectOption::label)
    }

    /// Deterministically looks up an option by stable value.
    #[must_use]
    pub fn option_for(&self, value: &str) -> Option<&NativeSelectOption> {
        self.options
            .iter()
            .find(|option| option.value.as_str() == value)
    }

    /// Returns the deterministic change intent for selecting `candidate`.
    ///
    /// Returns `None` when the control is disabled, the candidate equals the
    /// current value, no option carries that value, or the matching option is
    /// disabled.
    #[must_use]
    pub fn next_value_for(&self, candidate: &str) -> Option<NativeSelectChange> {
        if self.disabled {
            return None;
        }
        if candidate == self.value.as_str() {
            return None;
        }
        let option = self.option_for(candidate)?;
        if option.is_disabled() {
            return None;
        }
        Some(NativeSelectChange {
            value: option.value.clone(),
            label: option.label.clone(),
        })
    }

    /// Alias for [`Self::next_value_for`].
    #[must_use]
    pub fn change_for(&self, candidate: &str) -> Option<NativeSelectChange> {
        self.next_value_for(candidate)
    }

    /// Returns the current semantic decisions for caller composition/tests.
    #[must_use]
    pub fn semantics(&self) -> NativeSelectSemantics {
        let selected = self.selected_option();
        let has_value = selected.is_some() && !self.value.as_str().is_empty();
        let shows_placeholder = !has_value && self.placeholder.is_some();
        let selected_is_disabled = selected.is_some_and(NativeSelectOption::is_disabled);
        NativeSelectSemantics {
            flags: NativeSelectFlags::default()
                .with(NativeSelectFlags::DISABLED, self.disabled)
                .with(NativeSelectFlags::INVALID, self.invalid)
                .with(NativeSelectFlags::HAS_VALUE, has_value)
                .with(NativeSelectFlags::SHOWS_PLACEHOLDER, shows_placeholder)
                .with(NativeSelectFlags::FOCUSABLE, !self.disabled)
                .with(
                    NativeSelectFlags::HAS_DISABLED_SELECTION,
                    selected_is_disabled,
                ),
            value: self.value.clone(),
            selected_label: selected.map(|option| option.label.clone()),
            placeholder: self.placeholder.clone(),
            option_count: self.options.len(),
            selected_is_disabled,
            semantic_label: self.semantic_label.clone(),
            size: self.size,
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

impl Styled for NativeSelect {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

impl RenderOnce for NativeSelect {
    #[allow(clippy::too_many_lines)]
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let style = self.visual_style();

        let Self {
            id,
            focus,
            value,
            options,
            placeholder,
            disabled,
            focus_visibility,
            debug_selector,
            on_change,
            mut root,
            ..
        } = self;

        let selected = options
            .iter()
            .find(|option| option.value.as_str() == value.as_str());
        let has_value = selected.is_some() && !value.as_str().is_empty();
        let shows_placeholder = !has_value && placeholder.is_some();
        let text: SharedString = if let Some(option) = selected {
            option.label.clone()
        } else if let Some(placeholder) = placeholder.clone() {
            if has_value {
                // Unreachable placeholder path kept for exhaustiveness.
                value.clone()
            } else {
                placeholder
            }
        } else {
            // No selection and no placeholder: render empty branch.
            SharedString::from("")
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
            .child(text.clone());

        if shows_placeholder {
            content = content.text_color(style.placeholder_foreground);
        }
        if text.as_str().is_empty() && !shows_placeholder {
            // Ensure the empty branch still participates in layout with no text.
            content = content.child("");
        }

        let chevron = asset_glyph(artisan_assets::AssetId::TABLER_SELECTOR)
            .size(style.chevron_size)
            .text_color(style.chevron_color);

        let chevron_container = div()
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .size(style.chevron_size)
            .child(chevron);

        let root_selector = debug_selector.map_or_else(
            || DEFAULT_DEBUG_SELECTOR.to_owned(),
            |selector| selector.to_string(),
        );
        let branch_selector = format!("{root_selector}-{branch_name}");
        let chevron_selector = format!("{root_selector}-chevron");

        root = root.debug_selector(move || root_selector.clone());
        content = content.debug_selector(move || branch_selector.clone());
        let mut chevron_wrapped = div().debug_selector(move || chevron_selector.clone());
        chevron_wrapped = chevron_wrapped.child(chevron_container);

        if disabled {
            root = root.opacity(style.disabled_opacity);
        }

        // Compose the control row: value/placeholder label plus chevron.
        let control_row = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .flex_1()
            .min_w(px(0.0))
            .gap(px(8.0))
            .child(content)
            .child(chevron_wrapped);

        // If a change handler exists and the control is enabled, wire a simple
        // click-to-cycle behavior for the first enabled option that differs from
        // the current value. Full dropdown fidelity is not claimed; the
        // deterministic lookup API is the source of truth for option policy.
        let mut root = root.id(id).child(control_row);

        if disabled {
            return root;
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

        root = root.track_focus(&focus);

        if let Some(handler) = on_change {
            // The click handler emits a deterministic next value if one exists
            // beyond the current selection. This preserves a single honest
            // interaction without fabricating a native dropdown or claiming
            // platform select semantics.
            let options_for_handler = options.clone();
            let value_for_handler = value.clone();
            root = root.on_click(move |_, window, cx| {
                let current = value_for_handler.as_str();
                // Pick the first enabled option whose value differs from current.
                let Some(next) = options_for_handler
                    .iter()
                    .find(|option| !option.is_disabled() && option.value.as_str() != current)
                else {
                    return;
                };
                handler(
                    NativeSelectChange {
                        value: next.value.clone(),
                        label: next.label.clone(),
                    },
                    window,
                    cx,
                );
            });
        }

        root
    }
}

fn native_select_root(style: NativeSelectStyle) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .h(style.height)
        .pl(style.padding_left)
        .pr(style.padding_right)
        .py(style.padding_vertical)
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
