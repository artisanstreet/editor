//! Controlled segmented toggle groups for native GPUI settings controls.
//!
//! A toggle group is deliberately a render recipe. The selected value remains
//! owned by its caller; an interaction creates a [`ToggleGroupChange`] and
//! sends it to the caller without changing the component's retained selection.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::{
    AlignItems, App, BoxShadow, ClickEvent, Div, ElementId, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Pixels, RenderOnce, SharedString,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, point, px, transparent_black,
};

use crate::button::FocusVisibility;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};

const ITEM_CONTENT_GAP_PX: f32 = 4.0;
const DISABLED_OPACITY: f32 = 0.5;

/// Values accepted by a toggle group.
///
/// Hashing is required so that item debug selectors can identify a value
/// without placing arbitrary caller text in selectors or diagnostics.
pub trait ToggleValue: Clone + Eq + Hash + 'static {}

impl<T> ToggleValue for T where T: Clone + Eq + Hash + 'static {}

/// Selection behavior for a [`ToggleGroup`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ToggleGroupMode {
    /// At most one item is selected at a time.
    #[default]
    Single,
    /// Any number of items may be selected.
    Multiple,
}

/// Alias for the explicit selection mode used by a toggle group.
pub type ToggleGroupSelectionMode = ToggleGroupMode;

/// Short alias for [`ToggleGroupMode`].
pub type SelectionMode = ToggleGroupMode;

/// Main-axis direction used for roving keyboard navigation.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ToggleGroupOrientation {
    /// Items flow from left to right and Left/Right move focus.
    #[default]
    Horizontal,
    /// Items flow from top to bottom and Up/Down move focus.
    Vertical,
}

/// Short alias for [`ToggleGroupOrientation`].
pub type Orientation = ToggleGroupOrientation;

/// Visual treatment for a toggle group and its items.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ToggleGroupVariant {
    /// Chrome-free segmented controls with a transparent border.
    #[default]
    Default,
    /// Segmented controls with the shared input border.
    Outline,
}

/// Item dimensions on the shared control scale.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ToggleGroupSize {
    /// 36 px high with 12 px horizontal padding.
    #[default]
    Default,
    /// 32 px high with 12 px horizontal padding.
    Small,
    /// 40 px high with 16 px horizontal padding.
    Large,
}

/// The caller-owned selection passed to a [`ToggleGroup`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToggleGroupSelection<V = SharedString> {
    /// A single selected value, or no selected value.
    Single(Option<V>),
    /// Selected values in caller-provided order.
    Multiple(Vec<V>),
}

impl<V> Default for ToggleGroupSelection<V> {
    fn default() -> Self {
        Self::Single(None)
    }
}

impl<V> ToggleGroupSelection<V> {
    /// Creates a single-selection value.
    #[must_use]
    pub fn single(value: Option<V>) -> Self {
        Self::Single(value)
    }

    /// Creates a single-selection value with one selected item.
    #[must_use]
    pub fn selected(value: V) -> Self {
        Self::Single(Some(value))
    }

    /// Creates a multiple-selection value.
    #[must_use]
    pub fn multiple(values: impl IntoIterator<Item = V>) -> Self {
        Self::Multiple(values.into_iter().collect())
    }

    /// Returns the selection mode represented by this value.
    #[must_use]
    pub const fn mode(&self) -> ToggleGroupMode {
        match self {
            Self::Single(_) => ToggleGroupMode::Single,
            Self::Multiple(_) => ToggleGroupMode::Multiple,
        }
    }

    /// Returns whether `value` is currently selected.
    #[must_use]
    pub fn contains(&self, value: &V) -> bool
    where
        V: PartialEq,
    {
        match self {
            Self::Single(selected) => selected.as_ref() == Some(value),
            Self::Multiple(selected) => selected.iter().any(|item| item == value),
        }
    }

    /// Alias for [`Self::contains`].
    #[must_use]
    pub fn is_selected(&self, value: &V) -> bool
    where
        V: PartialEq,
    {
        self.contains(value)
    }

    /// Returns the selected value for single-selection groups.
    #[must_use]
    pub fn selected_value(&self) -> Option<&V> {
        match self {
            Self::Single(selected) => selected.as_ref(),
            Self::Multiple(_) => None,
        }
    }

    /// Returns whether no value is selected.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Single(selected) => selected.is_none(),
            Self::Multiple(selected) => selected.is_empty(),
        }
    }

    /// Returns the number of selected values.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Single(selected) => usize::from(selected.is_some()),
            Self::Multiple(selected) => selected.len(),
        }
    }
}

impl<V: Clone + PartialEq> ToggleGroupSelection<V> {
    /// Computes the next controlled selection for activating `value`.
    ///
    /// Single groups allow deselection by default, matching the legacy
    /// primitive. A caller that requires a persistent single selection can use
    /// [`ToggleGroup::allow_empty_selection`] with `false`.
    #[must_use]
    pub fn next_for(&self, value: &V, allow_empty_selection: bool) -> Self {
        match self {
            Self::Single(selected) => {
                if selected.as_ref() == Some(value) && allow_empty_selection {
                    Self::Single(None)
                } else {
                    Self::Single(Some(value.clone()))
                }
            }
            Self::Multiple(selected) => {
                let mut next = selected.clone();
                if let Some(index) = next.iter().position(|item| item == value) {
                    next.remove(index);
                } else {
                    next.push(value.clone());
                }
                Self::Multiple(next)
            }
        }
    }

    /// Alias for [`Self::next_for`].
    #[must_use]
    pub fn toggle(&self, value: &V, allow_empty_selection: bool) -> Self {
        self.next_for(value, allow_empty_selection)
    }
}

impl<V> From<Option<V>> for ToggleGroupSelection<V> {
    fn from(value: Option<V>) -> Self {
        Self::Single(value)
    }
}

impl<V> From<Vec<V>> for ToggleGroupSelection<V> {
    fn from(value: Vec<V>) -> Self {
        Self::Multiple(value)
    }
}

/// One controlled activation request emitted by a toggle group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToggleGroupChange<V = SharedString> {
    /// The item that was activated.
    pub value: V,
    /// Whether the activated item is pressed in the next selection.
    pub pressed: bool,
    /// The complete caller-owned selection to apply next.
    pub selection: ToggleGroupSelection<V>,
}

impl<V> ToggleGroupChange<V> {
    /// Constructs an activation request.
    #[must_use]
    pub const fn new(value: V, pressed: bool, selection: ToggleGroupSelection<V>) -> Self {
        Self {
            value,
            pressed,
            selection,
        }
    }

    /// Returns whether the activated item is pressed in the next selection.
    #[must_use]
    pub const fn is_pressed(&self) -> bool {
        self.pressed
    }

    /// Returns the complete controlled selection requested by this change.
    #[must_use]
    pub const fn next_selection(&self) -> &ToggleGroupSelection<V> {
        &self.selection
    }
}

/// Theme-resolved geometry and paint for a toggle group item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToggleGroupStyle {
    /// Item height.
    pub height: Pixels,
    /// Minimum item width.
    pub min_width: Pixels,
    /// Horizontal item padding.
    pub horizontal_padding: Pixels,
    /// Space between items.
    pub gap: Pixels,
    /// Radius of an independently spaced item.
    pub corner_radius: Pixels,
    /// Radius used only on the outside ends of joined items.
    pub joined_corner_radius: Pixels,
    /// Resting item background.
    pub background: gpui::Hsla,
    /// Resting item foreground.
    pub foreground: gpui::Hsla,
    /// Resting item border.
    pub border: gpui::Hsla,
    /// Selected and pressed background.
    pub selected_background: gpui::Hsla,
    /// Selected and pressed foreground.
    pub selected_foreground: gpui::Hsla,
    /// Hover background.
    pub hover_background: gpui::Hsla,
    /// Hover foreground.
    pub hover_foreground: gpui::Hsla,
    /// Full-alpha focus border.
    pub focus_border: gpui::Hsla,
    /// Focus ring paint.
    pub focus_ring: gpui::Hsla,
    /// Focus ring spread.
    pub focus_ring_width: Pixels,
    /// Disabled opacity.
    pub disabled_opacity: f32,
    /// Text size.
    pub text_size: Pixels,
}

impl ToggleGroupStyle {
    /// Resolves the segmented-control recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(
        theme: ArtisanTheme,
        variant: ToggleGroupVariant,
        size: ToggleGroupSize,
        spacing: Pixels,
    ) -> Self {
        let (height, min_width, horizontal_padding) = match size {
            ToggleGroupSize::Default => (
                theme.density.control_default,
                theme.density.control_default,
                px(12.0),
            ),
            ToggleGroupSize::Small => {
                (theme.density.control_sm, theme.density.control_sm, px(12.0))
            }
            ToggleGroupSize::Large => {
                (theme.density.control_lg, theme.density.control_lg, px(16.0))
            }
        };

        let transparent = transparent_black();
        let hover_background = match theme.mode {
            ThemeMode::Light => theme.colors.muted.to_paint(),
            ThemeMode::Dark => theme.colors.muted.with_alpha(0.5).to_paint(),
        };
        let border = match variant {
            ToggleGroupVariant::Default => transparent,
            ToggleGroupVariant::Outline => theme.colors.input.to_paint(),
        };

        Self {
            height,
            min_width,
            horizontal_padding,
            gap: normalize_spacing(spacing),
            corner_radius: RadiusTokens::value(RadiusStep::X4l),
            joined_corner_radius: RadiusTokens::value(RadiusStep::X3l),
            background: transparent,
            foreground: theme.colors.foreground.to_paint(),
            border,
            selected_background: theme.colors.muted.to_paint(),
            selected_foreground: theme.colors.foreground.to_paint(),
            hover_background,
            hover_foreground: theme.colors.foreground.to_paint(),
            focus_border: theme.colors.ring.to_paint(),
            focus_ring: theme.interaction.focus_ring_color.to_paint(),
            focus_ring_width: theme.interaction.focus_ring_width,
            disabled_opacity: DISABLED_OPACITY,
            text_size: theme.typography.control_text,
        }
    }

    /// Returns the height under the alternate item-height naming.
    #[must_use]
    pub const fn item_height(self) -> Pixels {
        self.height
    }

    /// Returns the minimum width under the alternate item-width naming.
    #[must_use]
    pub const fn item_min_width(self) -> Pixels {
        self.min_width
    }
}

/// A text-bearing item in a [`ToggleGroup`]. The value is the only semantic
/// identity used by selection; the label is presentation content.
#[derive(Clone)]
pub struct ToggleGroupItem<V: ToggleValue = SharedString> {
    value: V,
    label: SharedString,
    focus: FocusHandle,
    disabled: bool,
}

impl<V: ToggleValue> ToggleGroupItem<V> {
    /// Creates an item with a stable value, visible text, and focus handle.
    #[must_use]
    pub fn new(value: V, label: impl Into<SharedString>, focus: FocusHandle) -> Self {
        Self {
            value,
            label: label.into(),
            focus,
            disabled: false,
        }
    }

    /// Alternate constructor with the focus handle in the conventional second
    /// position used by the other native controls.
    #[must_use]
    pub fn with_focus(value: V, focus: FocusHandle, label: impl Into<SharedString>) -> Self {
        Self::new(value, label, focus)
    }

    /// Marks this item disabled or enabled.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the stable caller-owned value.
    #[must_use]
    pub const fn value(&self) -> &V {
        &self.value
    }

    /// Returns the visible text.
    #[must_use]
    pub fn label(&self) -> &str {
        self.label.as_ref()
    }

    /// Returns the focus handle used by roving focus.
    #[must_use]
    pub const fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    /// Returns whether this item is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// Callback invoked once for pointer activation and once for each complete
/// unmodified Enter/Space keyboard activation.
pub type ToggleGroupChangeHandler<V> =
    Rc<dyn Fn(ToggleGroupChange<V>, &ClickEvent, &mut Window, &mut App)>;

/// A controlled, segmented, roving-focus toggle group.
#[derive(IntoElement)]
pub struct ToggleGroup<V: ToggleValue = SharedString> {
    id: ElementId,
    theme: ArtisanTheme,
    selection: ToggleGroupSelection<V>,
    variant: ToggleGroupVariant,
    size: ToggleGroupSize,
    orientation: ToggleGroupOrientation,
    spacing: Pixels,
    disabled: bool,
    allow_empty_selection: bool,
    focus_visibility: FocusVisibility,
    items: Vec<ToggleGroupItem<V>>,
    on_change: Option<ToggleGroupChangeHandler<V>>,
    root: Div,
    debug_selector: Option<SharedString>,
}

impl<V: ToggleValue> ToggleGroup<V> {
    /// Constructs a group from an explicitly discriminated controlled
    /// selection. The default spacing is zero and the default orientation is
    /// horizontal.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        selection: ToggleGroupSelection<V>,
    ) -> Self {
        let mut group = Self {
            id: id.into(),
            theme,
            selection,
            variant: ToggleGroupVariant::Default,
            size: ToggleGroupSize::Default,
            orientation: ToggleGroupOrientation::Horizontal,
            spacing: px(0.0),
            disabled: false,
            allow_empty_selection: true,
            focus_visibility: FocusVisibility::Hidden,
            items: Vec::new(),
            on_change: None,
            root: div()
                .flex()
                .flex_row()
                .items_center()
                .flex_shrink_0()
                .flex_nowrap()
                .gap(px(0.0)),
            debug_selector: None,
        };
        group.apply_orientation();
        group
    }

    /// Constructs a single-selection group.
    #[must_use]
    pub fn single(id: impl Into<ElementId>, theme: ArtisanTheme, value: Option<V>) -> Self {
        Self::new(id, theme, ToggleGroupSelection::Single(value))
    }

    /// Constructs a multiple-selection group.
    #[must_use]
    pub fn multiple(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        Self::new(id, theme, ToggleGroupSelection::multiple(values))
    }

    /// Adds one item using a fluent builder.
    #[must_use]
    pub fn item(mut self, value: V, label: impl Into<SharedString>, focus: FocusHandle) -> Self {
        self.items.push(ToggleGroupItem::new(value, label, focus));
        self
    }

    /// Adds an already-built item using a fluent builder.
    #[must_use]
    pub fn with_item(mut self, item: ToggleGroupItem<V>) -> Self {
        self.items.push(item);
        self
    }

    /// Adds several already-built items using a fluent builder.
    #[must_use]
    pub fn with_items(mut self, items: impl IntoIterator<Item = ToggleGroupItem<V>>) -> Self {
        self.items.extend(items);
        self
    }

    /// Adds one item to a mutable group.
    pub fn push_item(&mut self, item: ToggleGroupItem<V>) {
        self.items.push(item);
    }

    /// Sets the visual variant.
    #[must_use]
    pub const fn variant(mut self, variant: ToggleGroupVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Sets the item size.
    #[must_use]
    pub const fn size(mut self, size: ToggleGroupSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the roving-focus axis and root layout direction.
    #[must_use]
    pub fn orientation(mut self, orientation: ToggleGroupOrientation) -> Self {
        self.orientation = orientation;
        self.apply_orientation();
        self
    }

    /// Sets the visual gap between items. Negative spacing is clamped to zero.
    #[must_use]
    pub fn spacing(mut self, spacing: impl Into<Pixels>) -> Self {
        self.spacing = normalize_spacing(spacing.into());
        self.root = self.root.gap(self.spacing);
        self
    }

    /// Sets spacing in the shared 4 px theme scale.
    #[must_use]
    pub fn spacing_steps(self, steps: f32) -> Self {
        let spacing = self.theme.spacing.steps(steps);
        self.spacing(spacing)
    }

    /// Selects the disabled presentation and suppresses all item interaction.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self.root = if disabled {
            self.root.opacity(DISABLED_OPACITY)
        } else {
            self.root.opacity(1.0)
        };
        self
    }

    /// Controls whether activating the selected single item may clear it.
    #[must_use]
    pub const fn allow_empty_selection(mut self, allow: bool) -> Self {
        self.allow_empty_selection = allow;
        self
    }

    /// Selects whether actual focus should receive a visible ring.
    #[must_use]
    pub const fn focus_visibility(mut self, visibility: FocusVisibility) -> Self {
        self.focus_visibility = visibility;
        self
    }

    /// Installs the controlled change callback.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(ToggleGroupChange<V>, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Alias for [`Self::on_change`].
    #[must_use]
    pub fn on_value_change(
        self,
        handler: impl Fn(ToggleGroupChange<V>, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change(handler)
    }

    /// Adds a stable debug selector to the group root.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the controlled selection retained by this render recipe.
    #[must_use]
    pub const fn selection(&self) -> &ToggleGroupSelection<V> {
        &self.selection
    }

    /// Returns this group's explicit selection mode.
    #[must_use]
    pub const fn mode(&self) -> ToggleGroupMode {
        self.selection.mode()
    }

    /// Returns the resolved style for the current group configuration.
    #[must_use]
    pub fn visual_style(&self) -> ToggleGroupStyle {
        ToggleGroupStyle::resolve(self.theme, self.variant, self.size, self.spacing)
    }

    /// Returns the configured spacing.
    #[must_use]
    pub const fn spacing_value(&self) -> Pixels {
        self.spacing
    }

    /// Returns whether this group is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns the configured item list.
    #[must_use]
    pub fn items(&self) -> &[ToggleGroupItem<V>] {
        &self.items
    }

    /// Returns whether an item value is selected in the retained selection.
    #[must_use]
    pub fn is_selected(&self, value: &V) -> bool {
        self.selection.contains(value)
    }

    fn apply_orientation(&mut self) {
        let mut root = std::mem::replace(&mut self.root, div());
        if self.orientation == ToggleGroupOrientation::Horizontal {
            root = root.flex_row().items_center();
        } else {
            root = root.flex_col();
            root.style().align_items = Some(AlignItems::Stretch);
        }
        self.root = root;
    }
}

impl<V: ToggleValue> Styled for ToggleGroup<V> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}
struct ToggleGroupRenderContext<'a, V: ToggleValue> {
    style: ToggleGroupStyle,
    selection: &'a ToggleGroupSelection<V>,
    variant: ToggleGroupVariant,
    orientation: ToggleGroupOrientation,
    group_disabled: bool,
    allow_empty_selection: bool,
    focus_visibility: FocusVisibility,
    on_change: &'a Option<ToggleGroupChangeHandler<V>>,
    item_selector_root: Option<&'a SharedString>,
    handles: Rc<Vec<FocusHandle>>,
    disabled_items: Rc<Vec<bool>>,
    roving_index: Option<usize>,
}

impl<V: ToggleValue> ToggleGroupRenderContext<'_, V> {
    fn render_item(&self, index: usize, item: ToggleGroupItem<V>) -> impl IntoElement + 'static {
        let ToggleGroupItem {
            value: item_value,
            label,
            focus: item_focus,
            ..
        } = item;
        let item_disabled = self.disabled_items[index];
        let selected = self.selection.contains(&item_value);
        let mut item_element = div()
            .id(&item_focus)
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .h(self.style.height)
            .min_w(self.style.min_width)
            .px(self.style.horizontal_padding)
            .gap(px(ITEM_CONTENT_GAP_PX))
            .rounded(self.style.corner_radius)
            .border_1()
            .border_color(self.style.border)
            .bg(if selected {
                self.style.selected_background
            } else {
                self.style.background
            })
            .text_color(if selected {
                self.style.selected_foreground
            } else {
                self.style.foreground
            })
            .text_size(self.style.text_size)
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap();

        item_element = apply_item_geometry(
            item_element,
            index,
            self.disabled_items.len(),
            self.orientation,
            self.variant,
            self.style,
        );
        if let Some(selector_root) = self.item_selector_root {
            let selector = item_debug_selector(selector_root.as_ref(), &item_value);
            item_element = item_element.debug_selector(move || selector);
        }
        item_element = item_element.child(label);

        if item_disabled {
            if !self.group_disabled {
                item_element = item_element.opacity(self.style.disabled_opacity);
            }
        } else {
            item_element =
                self.add_item_interactions(item_element, index, &item_focus, &item_value, selected);
        }
        item_element
    }

    fn add_item_interactions<E>(
        &self,
        mut item_element: E,
        index: usize,
        item_focus: &FocusHandle,
        item_value: &V,
        selected: bool,
    ) -> E
    where
        E: StatefulInteractiveElement + Styled,
    {
        let style = self.style;
        let focus_index = self.roving_index == Some(index);
        item_element = item_element
            .track_focus(item_focus)
            .tab_index(if focus_index { 0 } else { -1 })
            .hover(move |hover| {
                hover
                    .bg(if selected {
                        style.selected_background
                    } else {
                        style.hover_background
                    })
                    .text_color(if selected {
                        style.selected_foreground
                    } else {
                        style.hover_foreground
                    })
            })
            .active(move |active| {
                active
                    .bg(style.selected_background)
                    .text_color(style.selected_foreground)
            });

        if self.focus_visibility == FocusVisibility::Visible {
            item_element = item_element.focus(move |focused| {
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

        let handles_for_key = self.handles.clone();
        let disabled_for_key = self.disabled_items.clone();
        let orientation = self.orientation;
        item_element = item_element.on_key_down(move |event: &KeyDownEvent, window, cx| {
            if event.keystroke.modifiers.modified() {
                return;
            }
            if let Some(next) =
                navigation_target(index, &event.keystroke.key, orientation, &disabled_for_key)
            {
                window.focus(&handles_for_key[next]);
                window.prevent_default();
                cx.stop_propagation();
            }
        });

        if let Some(handler) = self.on_change.as_ref() {
            let callback = Rc::clone(handler);
            let next_selection = self
                .selection
                .next_for(item_value, self.allow_empty_selection);
            let change = ToggleGroupChange::new(
                item_value.clone(),
                next_selection.contains(item_value),
                next_selection,
            );
            item_element = item_element.on_click(move |event, window, cx| {
                callback(change.clone(), event, window, cx);
            });
        }
        item_element
    }
}

fn apply_item_geometry<E: Styled>(
    mut item_element: E,
    index: usize,
    item_count: usize,
    orientation: ToggleGroupOrientation,
    variant: ToggleGroupVariant,
    style: ToggleGroupStyle,
) -> E {
    if orientation == ToggleGroupOrientation::Vertical {
        item_element = item_element.w_full();
    }
    if style.gap == px(0.0) {
        item_element = item_element.rounded(px(0.0));
        if index == 0 {
            item_element = match orientation {
                ToggleGroupOrientation::Horizontal => {
                    item_element.rounded_l(style.joined_corner_radius)
                }
                ToggleGroupOrientation::Vertical => {
                    item_element.rounded_t(style.joined_corner_radius)
                }
            };
        }
        if index + 1 == item_count {
            item_element = match orientation {
                ToggleGroupOrientation::Horizontal => {
                    item_element.rounded_r(style.joined_corner_radius)
                }
                ToggleGroupOrientation::Vertical => {
                    item_element.rounded_b(style.joined_corner_radius)
                }
            };
        }
        if variant == ToggleGroupVariant::Outline && index > 0 {
            item_element = match orientation {
                ToggleGroupOrientation::Horizontal => item_element.border_l_0(),
                ToggleGroupOrientation::Vertical => item_element.border_t_0(),
            };
        }
    }
    item_element
}

fn item_handles<V: ToggleValue>(items: &[ToggleGroupItem<V>]) -> Rc<Vec<FocusHandle>> {
    Rc::new(
        items
            .iter()
            .map(|item| item.focus.clone())
            .collect::<Vec<_>>(),
    )
}

fn disabled_item_flags<V: ToggleValue>(
    items: &[ToggleGroupItem<V>],
    group_disabled: bool,
) -> Rc<Vec<bool>> {
    Rc::new(
        items
            .iter()
            .map(|item| group_disabled || item.disabled)
            .collect::<Vec<_>>(),
    )
}

fn find_roving_index<V: ToggleValue>(
    items: &[ToggleGroupItem<V>],
    disabled_items: &[bool],
    selection: &ToggleGroupSelection<V>,
    group_disabled: bool,
    window: &Window,
) -> Option<usize> {
    if group_disabled {
        return None;
    }
    items
        .iter()
        .enumerate()
        .find(|(index, item)| !disabled_items[*index] && item.focus.is_focused(window))
        .map(|(index, _)| index)
        .or_else(|| {
            items
                .iter()
                .enumerate()
                .find(|(index, item)| !disabled_items[*index] && selection.contains(&item.value))
                .map(|(index, _)| index)
        })
        .or_else(|| {
            items
                .iter()
                .enumerate()
                .find(|(index, _)| !disabled_items[*index])
                .map(|(index, _)| index)
        })
}

impl<V: ToggleValue> RenderOnce for ToggleGroup<V> {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ToggleGroup {
            id,
            theme,
            selection,
            variant,
            size,
            orientation,
            spacing,
            disabled,
            allow_empty_selection,
            focus_visibility,
            items,
            on_change,
            root,
            debug_selector,
        } = self;
        let style = ToggleGroupStyle::resolve(theme, variant, size, spacing);
        let handles = item_handles(&items);
        let disabled_items = disabled_item_flags(&items, disabled);
        let roving_index = find_roving_index(&items, &disabled_items, &selection, disabled, window);
        let root_selector = debug_selector;
        let context = ToggleGroupRenderContext {
            style,
            selection: &selection,
            variant,
            orientation,
            group_disabled: disabled,
            allow_empty_selection,
            focus_visibility,
            on_change: &on_change,
            item_selector_root: root_selector.as_ref(),
            handles,
            disabled_items,
            roving_index,
        };

        let mut root = root.id(id);
        if let Some(selector) = root_selector.as_ref() {
            let selector = selector.clone();
            root = root.debug_selector(move || selector.to_string());
        }
        for (index, item) in items.into_iter().enumerate() {
            root = root.child(context.render_item(index, item));
        }
        root
    }
}

fn normalize_spacing(spacing: Pixels) -> Pixels {
    px(f32::from(spacing).max(0.0))
}

fn navigation_target(
    current: usize,
    key: &str,
    orientation: ToggleGroupOrientation,
    disabled: &[bool],
) -> Option<usize> {
    if disabled.is_empty() || current >= disabled.len() {
        return None;
    }

    if key == "home" {
        return disabled.iter().position(|is_disabled| !is_disabled);
    }
    if key == "end" {
        return disabled.iter().rposition(|is_disabled| !is_disabled);
    }

    let forward = match (orientation, key) {
        (ToggleGroupOrientation::Horizontal, "right" | "arrowright")
        | (ToggleGroupOrientation::Vertical, "down" | "arrowdown") => true,
        (ToggleGroupOrientation::Horizontal, "left" | "arrowleft")
        | (ToggleGroupOrientation::Vertical, "up" | "arrowup") => false,
        _ => return None,
    };

    for offset in 1..=disabled.len() {
        let next = if forward {
            (current + offset) % disabled.len()
        } else {
            (current + disabled.len() - (offset % disabled.len())) % disabled.len()
        };
        if !disabled[next] {
            return Some(next);
        }
    }
    None
}

/// Returns a deterministic, value-derived selector suffix.
///
/// The value is fed through a local FNV-1a hasher rather than formatted with
/// `Debug` or `Display`, so arbitrary caller data never appears in selectors.
#[must_use]
pub fn stable_value_selector_suffix<V: Hash + ?Sized>(value: &V) -> String {
    let mut hasher = Fnv1aHasher(0xcbf2_9ce4_8422_2325);
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Builds the stable item selector used below a group root selector.
#[must_use]
pub fn item_debug_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    format!(
        "{root_selector}-item-{}",
        stable_value_selector_suffix(value)
    )
}

/// Alias for [`item_debug_selector`].
#[must_use]
pub fn toggle_group_item_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    item_debug_selector(root_selector, value)
}

struct Fnv1aHasher(u64);

impl Hasher for Fnv1aHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}
