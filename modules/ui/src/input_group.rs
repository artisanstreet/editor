//! Native GPUI input-group composition around a caller-owned control.
//!
//! Ported from the audited Svelte wrapper
//! (`modules/frontend/src/lib/components/ui/input-group/input-group.svelte`,
//! `input-group-addon.svelte`, `input-group-input.svelte`,
//! `input-group-text.svelte`) at the single configuration the product reaches:
//! a `group/input-group` flex container that composes leading and trailing
//! inline addons, optional block addons above and below the control, and a
//! single `input-group-control` child. The legacy presentation is
//! `border-input bg-surface-100 dark:bg-surface-900 h-9 rounded-4xl border`
//! with `has-[[data-slot=input-group-control]:focus-visible]:border-ring`,
//! `has-[[data-slot=input-group-control]:focus-visible]:ring-ring/50`,
//! `has-[[data-slot][aria-invalid=true]]:border-destructive`,
//! `has-[[data-slot][aria-invalid=true]]:ring-destructive/20`,
//! `has-data-[align=block-start]/has-data-[align=block-end]:rounded-2xl`,
//! and `has-[textarea]:rounded-xl`. This module keeps the same recipe without
//! inventing a browser input, focus-visible heuristic, or accessibility tree
//! that pinned GPUI 0.2.2 does not expose.
//!
//! The center control itself is never duplicated here. Callers retain their
//! buffered text in [`crate::input_state::TextInputState`], render a
//! [`crate::input::Input`] or any other `IntoElement`, and pass it as the
//! group's owned control. The group only owns chrome: border, background,
//! radius, focus/invalid/disabled presentation, and the deterministic ordering
//! of chrome slots around that control. No DOM shim or browser attribute is
//! synthesized.

use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, BoxShadow, Div, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, Refineable, RenderOnce, SharedString, StyleRefinement, Styled, Window,
    div, point, px,
};

pub use crate::button::FocusVisibility;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};

/// Stable selector used when a caller does not provide an instance name.
pub const DEFAULT_DEBUG_SELECTOR: &str = "artisan-input-group";

const BORDER_WIDTH_PX: f32 = 1.0;
const DISABLED_OPACITY: f32 = 0.5;

/// Alignment of one chrome slot relative to the owned control.
///
/// The Svelte source exposes `inline-start`, `inline-end`, `block-start`, and
/// `block-end` through `inputGroupAddonVariants`. This enum preserves exactly
/// that vocabulary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InputGroupAddonAlign {
    /// Leading inline chrome, `order-first` inside the row.
    InlineStart,
    /// Trailing inline chrome, `order-last` inside the row.
    InlineEnd,
    /// Full-width block chrome above the control, `order-first` and `w-full`.
    BlockStart,
    /// Full-width block chrome below the control, `order-last` and `w-full`.
    BlockEnd,
}

impl InputGroupAddonAlign {
    /// Returns the canonical token used in selectors and `data-align`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InlineStart => "inline-start",
            Self::InlineEnd => "inline-end",
            Self::BlockStart => "block-start",
            Self::BlockEnd => "block-end",
        }
    }

    /// Whether this slot participates in the inline row.
    #[must_use]
    pub const fn is_inline(self) -> bool {
        matches!(self, Self::InlineStart | Self::InlineEnd)
    }

    /// Whether this slot spans the full width above or below the row.
    #[must_use]
    pub const fn is_block(self) -> bool {
        matches!(self, Self::BlockStart | Self::BlockEnd)
    }

    /// Whether this slot renders before the control in the deterministic order.
    #[must_use]
    pub const fn is_leading(self) -> bool {
        matches!(self, Self::BlockStart | Self::InlineStart)
    }

    /// Deterministic rank used by the group's stable sort key.
    ///
    /// `BlockStart < InlineStart < control < InlineEnd < BlockEnd`.
    #[must_use]
    pub const fn order_key(self) -> u8 {
        match self {
            Self::BlockStart => 0,
            Self::InlineStart => 1,
            Self::InlineEnd => 2,
            Self::BlockEnd => 3,
        }
    }
}

/// Theme-resolved geometry and paint for one input group state.
///
/// The record is public so deterministic tests and future composition can
/// inspect the same recipe that the renderer applies. `border`,
/// `focus_border`, and `focus_ring` already include the invalid-state
/// decision. Radius follows the legacy Svelte policy verbatim:
/// `rounded-4xl` (26 px) for the default inline row, `rounded-2xl` (18 px)
/// when any block slot is present, and callers that compose a multi-line
/// textarea can map `has_block` to the `rounded-xl` (12 px) leg without this
/// module inventing text-area geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputGroupStyle {
    /// Default control height: the legacy `h-9`, 36 px. `None` when a block
    /// slot forces the group to `h-auto` / `flex-col`.
    pub height: Option<Pixels>,
    /// Corner radius for this group's current block configuration.
    pub corner_radius: Pixels,
    /// One-pixel group border.
    pub border_width: Pixels,
    /// Theme surface 100 in light mode and surface 900 in dark mode.
    pub background: Hsla,
    /// Control value text color (`--foreground`).
    pub foreground: Hsla,
    /// Dim chrome text color (`--muted-foreground`).
    pub muted_foreground: Hsla,
    /// Current border, including the invalid-state branch.
    pub border: Hsla,
    /// Border color while the explicitly visible focus state is active.
    pub focus_border: Hsla,
    /// Ring color while the explicitly visible focus state is active.
    pub focus_ring: Hsla,
    /// Focus ring width: the legacy `[3px]` treatment.
    pub focus_ring_width: Pixels,
    /// Disabled presentation opacity from the legacy wrapper.
    pub disabled_opacity: f32,
    /// Whether this recipe was resolved for an invalid value.
    pub invalid: bool,
    /// Whether this recipe was resolved with at least one block slot.
    pub has_block: bool,
}

impl InputGroupStyle {
    /// Resolves the reached input-group recipe from the shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, invalid: bool, has_block: bool) -> Self {
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

        let corner_radius = if has_block {
            RadiusTokens::value(RadiusStep::X2l)
        } else {
            RadiusTokens::value(RadiusStep::X4l)
        };

        Self {
            height: if has_block {
                None
            } else {
                Some(theme.density.control_default)
            },
            corner_radius,
            border_width: px(BORDER_WIDTH_PX),
            background: theme.surfaces.value(surface).to_paint(),
            foreground: theme.colors.foreground.to_paint(),
            muted_foreground: theme.colors.muted_foreground.to_paint(),
            border,
            focus_border,
            focus_ring,
            focus_ring_width: theme.interaction.focus_ring_width,
            disabled_opacity: DISABLED_OPACITY,
            invalid,
            has_block,
        }
    }
}

/// Compact flags for the caller-visible semantic state of one input group.
///
/// GPUI 0.2.2 does not expose a platform accessibility tree. These flags
/// retain the deterministic state decisions needed by composition and tests
/// without presenting them as platform semantics.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct InputGroupFlags(u8);

impl InputGroupFlags {
    /// The group is disabled.
    pub const DISABLED: Self = Self(0b0000_0001);
    /// The group is invalid.
    pub const INVALID: Self = Self(0b0000_0010);
    /// The group contains at least one block slot.
    pub const HAS_BLOCK: Self = Self(0b0000_0100);
    /// The group contains at least one inline slot.
    pub const HAS_INLINE: Self = Self(0b0000_1000);
    /// The group participates in GPUI focus tracking.
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

    /// Returns whether the group is disabled.
    #[must_use]
    pub const fn is_disabled(self) -> bool {
        self.contains(Self::DISABLED)
    }

    /// Returns whether the group is invalid.
    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.contains(Self::INVALID)
    }

    /// Returns whether the group has a block slot.
    #[must_use]
    pub const fn has_block(self) -> bool {
        self.contains(Self::HAS_BLOCK)
    }

    /// Returns whether the group has an inline slot.
    #[must_use]
    pub const fn has_inline(self) -> bool {
        self.contains(Self::HAS_INLINE)
    }

    /// Returns whether the group participates in GPUI focus tracking.
    #[must_use]
    pub const fn is_focusable(self) -> bool {
        self.contains(Self::FOCUSABLE)
    }
}

/// Caller-visible semantic state retained without claiming platform
/// accessibility support that GPUI 0.2.2 does not expose.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InputGroupSemantics {
    /// Compact disabled, invalid, block, inline, and focusability flags.
    pub flags: InputGroupFlags,
    /// Optional caller-provided label metadata for future accessibility
    /// wiring; this value is not registered with a platform accessibility API.
    pub semantic_label: Option<SharedString>,
}

struct AddonEntry {
    align: InputGroupAddonAlign,
    label: SharedString,
    content: AnyElement,
}

/// A native GPUI input-group container that composes chrome slots around a
/// caller-owned control.
///
/// The control is supplied as an arbitrary [`IntoElement`] value so existing
/// primitives such as [`crate::input::Input`] compose without being
/// duplicated or reimplemented. Addons are typed by
/// [`InputGroupAddonAlign`] and are always rendered in the deterministic
/// order `BlockStart → InlineStart → control → InlineEnd → BlockEnd`, stable
/// under insertion order.
#[derive(IntoElement)]
pub struct InputGroup {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    disabled: bool,
    invalid: bool,
    focus_visibility: FocusVisibility,
    semantic_label: Option<SharedString>,
    debug_selector: Option<SharedString>,
    control: AnyElement,
    control_label: SharedString,
    addons: Vec<AddonEntry>,
    root: Div,
}

impl InputGroup {
    /// Constructs an input group around a caller-owned control.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        control: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            disabled: false,
            invalid: false,
            focus_visibility: FocusVisibility::Hidden,
            semantic_label: None,
            debug_selector: Some(DEFAULT_DEBUG_SELECTOR.into()),
            control: control.into_any_element(),
            control_label: SharedString::from("control"),
            addons: Vec::new(),
            root: div(),
        }
    }

    /// Appends one chrome slot at the requested alignment.
    ///
    /// The `label` is retained for deterministic debug selectors and ordering
    /// assertions. Content is the caller's GPUI element (text, icon, button).
    #[must_use]
    pub fn addon(
        mut self,
        align: InputGroupAddonAlign,
        label: impl Into<SharedString>,
        content: impl IntoElement,
    ) -> Self {
        self.addons.push(AddonEntry {
            align,
            label: label.into(),
            content: content.into_any_element(),
        });
        self
    }

    /// Convenience for a leading inline slot (`InlineStart`).
    #[must_use]
    pub fn leading_addon(self, label: impl Into<SharedString>, content: impl IntoElement) -> Self {
        self.addon(InputGroupAddonAlign::InlineStart, label, content)
    }

    /// Convenience for a trailing inline slot (`InlineEnd`).
    #[must_use]
    pub fn trailing_addon(self, label: impl Into<SharedString>, content: impl IntoElement) -> Self {
        self.addon(InputGroupAddonAlign::InlineEnd, label, content)
    }

    /// Convenience for a block slot rendered above the inline row.
    #[must_use]
    pub fn block_start(self, label: impl Into<SharedString>, content: impl IntoElement) -> Self {
        self.addon(InputGroupAddonAlign::BlockStart, label, content)
    }

    /// Convenience for a block slot rendered below the inline row.
    #[must_use]
    pub fn block_end(self, label: impl Into<SharedString>, content: impl IntoElement) -> Self {
        self.addon(InputGroupAddonAlign::BlockEnd, label, content)
    }

    /// Overrides the debug selector label used for the control slot.
    #[must_use]
    pub fn control_label(mut self, label: impl Into<SharedString>) -> Self {
        self.control_label = label.into();
        self
    }

    /// Sets the invalid visual and semantic state.
    #[must_use]
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
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

    /// Adds a stable GPUI debug selector to the root and its slots.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns whether the group has at least one block slot.
    #[must_use]
    pub fn has_block(&self) -> bool {
        self.addons.iter().any(|addon| addon.align.is_block())
    }

    /// Returns whether the group has at least one inline slot.
    #[must_use]
    pub fn has_inline(&self) -> bool {
        self.addons.iter().any(|addon| addon.align.is_inline())
    }

    /// Returns the theme-resolved visual recipe for the current state.
    #[must_use]
    pub fn visual_style(&self) -> InputGroupStyle {
        InputGroupStyle::resolve(self.theme, self.invalid, self.has_block())
    }

    /// Returns whether this group is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns whether this group is invalid.
    #[must_use]
    pub const fn is_invalid(&self) -> bool {
        self.invalid
    }

    /// Returns the number of registered addon slots.
    #[must_use]
    pub fn addon_count(&self) -> usize {
        self.addons.len()
    }

    /// Returns the addon alignments in insertion order.
    #[must_use]
    pub fn addon_aligns(&self) -> Vec<InputGroupAddonAlign> {
        self.addons.iter().map(|entry| entry.align).collect()
    }

    /// Returns the addon `data-align` tokens in insertion order.
    #[must_use]
    pub fn addon_align_tokens(&self) -> Vec<&'static str> {
        self.addons
            .iter()
            .map(|entry| entry.align.as_str())
            .collect()
    }

    /// Returns the addon alignments in deterministic render order.
    ///
    /// The order is `BlockStart → InlineStart → InlineEnd → BlockEnd`, stable
    /// within each rank.
    #[must_use]
    pub fn ordered_addon_aligns(&self) -> Vec<InputGroupAddonAlign> {
        let mut indices: Vec<usize> = (0..self.addons.len()).collect();
        indices.sort_by_key(|index| (self.addons[*index].align.order_key(), *index));
        indices
            .into_iter()
            .map(|index| self.addons[index].align)
            .collect()
    }

    /// Returns the addon labels in deterministic render order.
    #[must_use]
    pub fn ordered_addon_labels(&self) -> Vec<SharedString> {
        let mut indices: Vec<usize> = (0..self.addons.len()).collect();
        indices.sort_by_key(|index| (self.addons[*index].align.order_key(), *index));
        indices
            .into_iter()
            .map(|index| self.addons[index].label.clone())
            .collect()
    }

    /// Returns the current semantic decisions for caller composition/tests.
    #[must_use]
    pub fn semantics(&self) -> InputGroupSemantics {
        InputGroupSemantics {
            flags: InputGroupFlags::default()
                .with(InputGroupFlags::DISABLED, self.disabled)
                .with(InputGroupFlags::INVALID, self.invalid)
                .with(InputGroupFlags::HAS_BLOCK, self.has_block())
                .with(InputGroupFlags::HAS_INLINE, self.has_inline())
                .with(InputGroupFlags::FOCUSABLE, !self.disabled),
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

impl Styled for InputGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

/// Addons partitioned by alignment slot.
struct OrderedInputGroupAddons {
    block_start: Vec<AddonEntry>,
    inline_start: Vec<AddonEntry>,
    inline_end: Vec<AddonEntry>,
    block_end: Vec<AddonEntry>,
}

/// Orders addons deterministically and partitions them by alignment slot.
fn order_input_group_addons(addons: Vec<AddonEntry>) -> OrderedInputGroupAddons {
    // Deterministic ordering without moving callers' insertion vector.
    let mut ordered_indices: Vec<usize> = (0..addons.len()).collect();
    ordered_indices.sort_by_key(|index| (addons[*index].align.order_key(), *index));

    // Take ownership of entries in deterministic order so `AnyElement` can move.
    let mut ordered_addons: Vec<AddonEntry> = Vec::with_capacity(addons.len());
    let mut addons_opt: Vec<Option<AddonEntry>> = addons.into_iter().map(Some).collect();
    for index in ordered_indices {
        if let Some(entry) = addons_opt[index].take() {
            ordered_addons.push(entry);
        }
    }

    let mut ordered = OrderedInputGroupAddons {
        block_start: Vec::new(),
        inline_start: Vec::new(),
        inline_end: Vec::new(),
        block_end: Vec::new(),
    };
    for entry in ordered_addons {
        match entry.align {
            InputGroupAddonAlign::BlockStart => ordered.block_start.push(entry),
            InputGroupAddonAlign::InlineStart => ordered.inline_start.push(entry),
            InputGroupAddonAlign::InlineEnd => ordered.inline_end.push(entry),
            InputGroupAddonAlign::BlockEnd => ordered.block_end.push(entry),
        }
    }
    ordered
}

/// Owned inputs for one addon chrome wrapper.
struct InputGroupAddonFrame<'a> {
    entry: AddonEntry,
    theme: &'a ArtisanTheme,
    root_selector: &'a str,
    muted: Hsla,
    block_px: Pixels,
}

/// Wraps one owned addon entry in its chrome.
fn render_input_group_addon(frame: InputGroupAddonFrame<'_>) -> Div {
    let InputGroupAddonFrame {
        entry,
        theme,
        root_selector,
        muted,
        block_px,
    } = frame;
    let selector = format!(
        "{root_selector}-addon-{}-{}",
        entry.align.as_str(),
        entry.label
    );
    let is_block = entry.align.is_block();
    let align = entry.align;
    let content = entry.content;
    let mut element = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(theme.spacing.steps(1.0))
        .text_color(muted)
        .text_size(theme.typography.control_text)
        .debug_selector(move || selector.clone());
    if is_block {
        element = element
            .w_full()
            .justify_start()
            .px(block_px)
            .py(theme.spacing.steps(2.0));
        if align == InputGroupAddonAlign::BlockStart {
            element = element.pt(block_px);
        } else {
            element = element.pb(block_px);
        }
    } else if align == InputGroupAddonAlign::InlineStart {
        element = element.pl(block_px);
    } else {
        element = element.pr(block_px);
    }
    element.child(content)
}

/// Owned inputs for the inline row (inline addons plus control slot).
struct InputGroupInlineRowFrame<'a> {
    inline_start: Vec<AddonEntry>,
    inline_end: Vec<AddonEntry>,
    control: AnyElement,
    control_selector: String,
    theme: &'a ArtisanTheme,
    root_selector: &'a str,
    muted: Hsla,
    block_px: Pixels,
    inline_gap: Pixels,
}

/// Assembles the inline row that owns the caller control exactly once.
fn render_input_group_inline_row(frame: InputGroupInlineRowFrame<'_>) -> Div {
    let InputGroupInlineRowFrame {
        inline_start,
        inline_end,
        control,
        control_selector,
        theme,
        root_selector,
        muted,
        block_px,
        inline_gap,
    } = frame;
    // The inline row is the only place the caller-owned control lives, so the
    // block column and the inline row share it exactly once.
    let mut inline_row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(inline_gap)
        .w_full()
        .min_w_0()
        .flex_1();

    for entry in inline_start {
        inline_row = inline_row.child(render_input_group_addon(InputGroupAddonFrame {
            entry,
            theme,
            root_selector,
            muted,
            block_px,
        }));
    }

    let control_slot = div()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .debug_selector({
            let selector = control_selector.clone();
            move || selector.clone()
        })
        .child(control);
    inline_row = inline_row.child(control_slot);

    for entry in inline_end {
        inline_row = inline_row.child(render_input_group_addon(InputGroupAddonFrame {
            entry,
            theme,
            root_selector,
            muted,
            block_px,
        }));
    }
    inline_row
}

/// Owned inputs for the group chrome wrapper.
struct InputGroupChromeFrame {
    style: InputGroupStyle,
    has_block: bool,
    root_selector: String,
    content: Div,
}

/// Wraps assembled content in the group chrome.
fn render_input_group_chrome(frame: InputGroupChromeFrame) -> Div {
    let InputGroupChromeFrame {
        style,
        has_block,
        root_selector,
        content,
    } = frame;
    div()
        .flex()
        .when(has_block, Styled::flex_col)
        .when(!has_block, |element| element.flex_row().items_center())
        .w_full()
        .min_w_0()
        .rounded(style.corner_radius)
        .border(style.border_width)
        .border_color(style.border)
        .bg(style.background)
        .text_color(style.foreground)
        .overflow_hidden()
        .when_some(style.height, Styled::h)
        .when(has_block, |element| {
            element.min_h(style.height.unwrap_or(px(36.0)))
        })
        .debug_selector(move || root_selector.clone())
        .child(content)
}

impl RenderOnce for InputGroup {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let style = InputGroupStyle::resolve(self.theme, self.invalid, {
            self.addons.iter().any(|addon| addon.align.is_block())
        });

        let Self {
            id,
            focus,
            theme,
            disabled,
            focus_visibility,
            debug_selector,
            control,
            control_label,
            addons,
            mut root,
            ..
        } = self;

        let has_block = style.has_block;
        let root_selector = debug_selector
            .as_ref()
            .map_or_else(|| DEFAULT_DEBUG_SELECTOR.to_owned(), ToString::to_string);
        let control_selector = format!("{root_selector}-control-{control_label}");
        let muted = style.muted_foreground;

        let ordered = order_input_group_addons(addons);
        let OrderedInputGroupAddons {
            block_start,
            inline_start,
            inline_end,
            block_end,
        } = ordered;

        let inline_gap = theme.spacing.steps(2.0);
        let block_px = theme.spacing.steps(3.0);

        let inline_row = render_input_group_inline_row(InputGroupInlineRowFrame {
            inline_start,
            inline_end,
            control,
            control_selector,
            theme: &theme,
            root_selector: &root_selector,
            muted,
            block_px,
            inline_gap,
        });

        let content: Div = if has_block {
            let mut column = div().flex().flex_col().w_full().min_w_0();
            for entry in block_start {
                column = column.child(render_input_group_addon(InputGroupAddonFrame {
                    entry,
                    theme: &theme,
                    root_selector: &root_selector,
                    muted,
                    block_px,
                }));
            }
            column = column.child(inline_row);
            for entry in block_end {
                column = column.child(render_input_group_addon(InputGroupAddonFrame {
                    entry,
                    theme: &theme,
                    root_selector: &root_selector,
                    muted,
                    block_px,
                }));
            }
            column
        } else {
            inline_row
        };

        // Group chrome.
        let mut group = render_input_group_chrome(InputGroupChromeFrame {
            style,
            has_block,
            root_selector: root_selector.clone(),
            content,
        });

        // Caller refinements win.
        group.style().refine(root.style());

        if disabled {
            group = group.opacity(style.disabled_opacity);
        }

        let mut group = group.id(id);
        if disabled {
            return group;
        }

        if focus_visibility == FocusVisibility::Visible {
            group = group.focus(move |focused| {
                focused
                    .border_color(style.focus_border)
                    .shadow(vec![BoxShadow {
                        color: style.focus_ring,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: style.focus_ring_width,
                        inset: false,
                    }])
            });
        }

        group.track_focus(&focus)
    }
}
