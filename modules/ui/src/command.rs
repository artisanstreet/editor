//! Native command-palette and command-list presentation for GPUI.
//!
//! This is a controlled presentation leaf. The caller owns the query and
//! supplies groups that have already been filtered and ordered by its domain
//! policy. In particular, this module does not import or reproduce the
//! frontend command matcher, inspect searchable metadata, or execute a
//! command. Its responsibility is the stable-ID presentation and the small
//! amount of keyboard/pointer interaction that belongs to the list itself.
//!
//! Pinned GPUI 0.2.2 has focus handles and event dispatch, but no command
//! widget or platform accessibility tree. The palette therefore retains
//! semantic state and focus policy as ordinary Rust data while being honest
//! that those values are not an OS accessibility tree.

use std::cell::RefCell;
use std::rc::Rc;

use artisan_assets::AssetId;
use gpui::prelude::Refineable;
use gpui::{
    App, BoxShadow, Div, ElementId, FocusHandle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Pixels, RenderOnce, SharedString, Stateful, StatefulInteractiveElement,
    StyleRefinement, Styled, Window, div, point, px,
};

pub use crate::button::FocusVisibility;
use crate::icon::{IconSize, IconStyle, IconTint, icon as render_icon};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};

/// The stable selector used when a caller does not provide an instance name.
pub const DEFAULT_DEBUG_SELECTOR: &str = "artisan-command-palette";

/// Semantic role retained for a future accessibility adapter.
///
/// GPUI 0.2.2 does not emit an accessibility tree, so this is metadata only.
pub const COMMAND_PALETTE_ROLE: &str = "application";

/// Default empty-state label.
pub const DEFAULT_EMPTY_LABEL: &str = "No results found.";
const DISABLED_OPACITY: f32 = 0.5;
const SEPARATOR_OPACITY: f32 = 0.5;

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

/// One stable-ID command row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandItem {
    /// Stable command identity. It is never derived from the row position.
    pub id: SharedString,
    /// Text rendered in the row.
    pub label: SharedString,
    /// Optional catalog-backed leading icon.
    pub icon: Option<AssetId>,
    /// Optional keyboard hint rendered at the trailing edge.
    pub shortcut: Option<SharedString>,
    /// Disabled rows remain visible but cannot be highlighted or activated.
    pub disabled: bool,
}

impl CommandItem {
    /// Creates an enabled command without optional chrome.
    #[must_use]
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            shortcut: None,
            disabled: false,
        }
    }

    /// Supplies the catalog-backed leading icon.
    #[must_use]
    pub const fn with_icon(mut self, icon: AssetId) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Alias for [`Self::with_icon`] for fluent call sites.
    #[must_use]
    pub const fn icon(self, icon: AssetId) -> Self {
        self.with_icon(icon)
    }

    /// Supplies the trailing keyboard shortcut label.
    #[must_use]
    pub fn with_shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Alias for [`Self::with_shortcut`] for fluent call sites.
    #[must_use]
    pub fn shortcut(self, shortcut: impl Into<SharedString>) -> Self {
        self.with_shortcut(shortcut)
    }

    /// Selects whether this row is disabled.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns whether this row is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// An ordered command group with an optional visible heading.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandGroup {
    /// Stable group identity used with the item identity for element IDs.
    pub id: SharedString,
    /// Optional heading rendered above this group's rows.
    pub heading: Option<SharedString>,
    /// Rows in the caller-supplied order.
    pub items: Vec<CommandItem>,
}

impl CommandGroup {
    /// Creates a group with no heading.
    #[must_use]
    pub fn new(id: impl Into<SharedString>, items: Vec<CommandItem>) -> Self {
        Self {
            id: id.into(),
            heading: None,
            items,
        }
    }

    /// Supplies the visible heading.
    #[must_use]
    pub fn heading(mut self, heading: impl Into<SharedString>) -> Self {
        self.heading = Some(heading.into());
        self
    }

    /// Returns whether this group has no rows.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Returns this group's rows in caller-supplied order.
    #[must_use]
    pub fn items(&self) -> &[CommandItem] {
        &self.items
    }
}

/// The stable identity and label reported for one activation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandActivation {
    /// Stable group identity of the activated row.
    pub group_id: SharedString,
    /// Stable item identity of the activated row.
    pub item_id: SharedString,
    /// Label snapshot associated with the activation.
    pub label: SharedString,
}

impl CommandActivation {
    /// Creates an activation record from the owning group and item.
    #[must_use]
    pub fn new(
        group_id: impl Into<SharedString>,
        item_id: impl Into<SharedString>,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            group_id: group_id.into(),
            item_id: item_id.into(),
            label: label.into(),
        }
    }
}

/// Semantic state retained by the palette for a future accessibility layer.
///
/// These values describe the rendered primitive only. They do not claim that
/// GPUI has emitted an OS accessibility tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandSemantics {
    /// The retained semantic role.
    pub role: &'static str,
    /// Number of supplied rows, including disabled rows.
    pub item_count: usize,
    /// Stable ID of the resolved highlighted row, if any.
    pub highlighted_id: Option<SharedString>,
    /// Whether the caller requested a visible focus treatment.
    pub focus_visibility: FocusVisibility,
}

// ---------------------------------------------------------------------------
// Theme-resolved style
// ---------------------------------------------------------------------------

/// Theme-resolved geometry and paint for the command palette.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CommandStyle {
    /// Palette padding (`p-1`).
    pub outer_padding: Pixels,
    /// Palette corners (`rounded-4xl`).
    pub outer_radius: Pixels,
    /// Popover surface.
    pub background: Hsla,
    /// Popover foreground.
    pub foreground: Hsla,
    /// Muted foreground for headings, placeholders, shortcuts, and empty text.
    pub muted_foreground: Hsla,
    /// Input border token.
    pub input_border: Hsla,
    /// Input background: surface 100 in light mode and surface 900 in dark.
    pub input_background: Hsla,
    /// Input height (`h-9`).
    pub input_height: Pixels,
    /// Input horizontal padding.
    pub input_horizontal_padding: Pixels,
    /// Search-icon/query gap.
    pub input_gap: Pixels,
    /// Search icon edge (`size-4`).
    pub input_icon_size: Pixels,
    /// Input and row text size (`text-sm`).
    pub item_text_size: Pixels,
    /// Single-line text leading for input and rows.
    pub item_line_height: Pixels,
    /// Search icon opacity from the reached input chrome.
    pub search_icon_opacity: f32,
    /// Maximum scroll-list height (`max-h-72`, resolved from the theme).
    pub list_max_height: Pixels,
    /// List scroll padding (`scroll-py-1`).
    pub list_scroll_padding: Pixels,
    /// Row horizontal padding (`px-2`).
    pub item_horizontal_padding: Pixels,
    /// Row vertical padding (`py-1.5`).
    pub item_vertical_padding: Pixels,
    /// Row corners (`rounded-sm`).
    pub item_corner_radius: Pixels,
    /// Gap between row children (`gap-2`).
    pub item_gap: Pixels,
    /// Group heading text size (`text-xs`).
    pub heading_text_size: Pixels,
    /// Group heading horizontal padding.
    pub heading_horizontal_padding: Pixels,
    /// Group heading vertical padding.
    pub heading_vertical_padding: Pixels,
    /// Group heading weight.
    pub heading_weight: gpui::FontWeight,
    /// Shortcut text size (`text-xs`).
    pub shortcut_text_size: Pixels,
    /// Highlight fill (`bg-muted`).
    pub highlight_background: Hsla,
    /// Highlight text (`text-foreground`).
    pub highlight_foreground: Hsla,
    /// Decorative separator color.
    pub separator: Hsla,
    /// Decorative separator height.
    pub separator_height: Pixels,
    /// Decorative separator vertical margin.
    pub separator_margin: Pixels,
    /// Empty-state text size (`text-sm`).
    pub empty_text_size: Pixels,
    /// Empty-state vertical padding.
    pub empty_vertical_padding: Pixels,
    /// Disabled row opacity.
    pub disabled_opacity: f32,
    /// Focus-ring paint.
    pub focus_ring: Hsla,
    /// Focus-ring spread.
    pub focus_ring_width: Pixels,
}

impl CommandStyle {
    /// Resolves the palette style exclusively from existing theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        let input_surface = match theme.mode {
            ThemeMode::Light => SurfaceStep::S100,
            ThemeMode::Dark => SurfaceStep::S900,
        };

        Self {
            outer_padding: theme.spacing.steps(1.0),
            outer_radius: RadiusTokens::value(RadiusStep::X4l),
            background: theme.colors.popover.to_paint(),
            foreground: theme.colors.popover_foreground.to_paint(),
            muted_foreground: theme.colors.muted_foreground.to_paint(),
            input_border: theme.colors.input.to_paint(),
            input_background: theme.surfaces.value(input_surface).to_paint(),
            input_height: theme.density.control_default,
            input_horizontal_padding: theme.spacing.steps(3.0),
            input_gap: theme.spacing.steps(2.0),
            input_icon_size: theme.spacing.steps(4.0),
            item_text_size: theme.typography.control_text,
            item_line_height: theme.spacing.steps(5.0),
            search_icon_opacity: SEPARATOR_OPACITY,
            list_max_height: theme.density.command_list_max_height,
            list_scroll_padding: theme.spacing.steps(1.0),
            item_horizontal_padding: theme.spacing.steps(2.0),
            item_vertical_padding: theme.spacing.steps(1.5),
            item_corner_radius: RadiusTokens::value(RadiusStep::Sm),
            item_gap: theme.spacing.steps(2.0),
            heading_text_size: theme.typography.label_text,
            heading_horizontal_padding: theme.spacing.steps(2.0),
            heading_vertical_padding: theme.spacing.steps(1.5),
            heading_weight: gpui::FontWeight::MEDIUM,
            shortcut_text_size: theme.typography.label_text,
            highlight_background: theme.colors.muted.to_paint(),
            highlight_foreground: theme.colors.foreground.to_paint(),
            separator: theme.colors.border.with_alpha(SEPARATOR_OPACITY).to_paint(),
            separator_height: px(1.0),
            separator_margin: theme.spacing.steps(1.0),
            empty_text_size: theme.typography.control_text,
            empty_vertical_padding: theme.spacing.steps(6.0),
            disabled_opacity: DISABLED_OPACITY,
            focus_ring: theme.interaction.focus_ring_color.to_paint(),
            focus_ring_width: theme.interaction.focus_ring_width,
        }
    }
}

// ---------------------------------------------------------------------------
// Stable-ID resolution and navigation
// ---------------------------------------------------------------------------

fn visible_group_indices(groups: &[CommandGroup]) -> Vec<usize> {
    groups
        .iter()
        .enumerate()
        .filter_map(|(index, group)| (!group.items.is_empty()).then_some(index))
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ItemLocation {
    group: usize,
    item: usize,
}

fn item_at(groups: &[CommandGroup], location: ItemLocation) -> Option<&CommandItem> {
    groups
        .get(location.group)
        .and_then(|group| group.items.get(location.item))
}

fn enabled_locations(groups: &[CommandGroup]) -> Vec<ItemLocation> {
    groups
        .iter()
        .enumerate()
        .flat_map(|(group, value)| {
            value
                .items
                .iter()
                .enumerate()
                .filter_map(move |(item, value)| {
                    (!value.disabled).then_some(ItemLocation { group, item })
                })
        })
        .collect()
}

fn enabled_location_for_id(groups: &[CommandGroup], id: &SharedString) -> Option<ItemLocation> {
    groups.iter().enumerate().find_map(|(group, value)| {
        value
            .items
            .iter()
            .enumerate()
            .find(|(_, item)| !item.disabled && item.id == *id)
            .map(|(item, _)| ItemLocation { group, item })
    })
}

fn location_id(groups: &[CommandGroup], location: ItemLocation) -> Option<SharedString> {
    item_at(groups, location).map(|item| item.id.clone())
}

fn first_enabled_id(groups: &[CommandGroup]) -> Option<SharedString> {
    enabled_locations(groups)
        .first()
        .and_then(|location| location_id(groups, *location))
}

fn last_enabled_id(groups: &[CommandGroup]) -> Option<SharedString> {
    enabled_locations(groups)
        .last()
        .and_then(|location| location_id(groups, *location))
}

fn resolved_highlight_id(
    groups: &[CommandGroup],
    requested: Option<&SharedString>,
) -> Option<SharedString> {
    requested
        .and_then(|id| enabled_location_for_id(groups, id))
        .and_then(|location| location_id(groups, location))
        .or_else(|| first_enabled_id(groups))
}

fn next_or_previous_id(
    groups: &[CommandGroup],
    current: Option<&SharedString>,
    forward: bool,
) -> Option<SharedString> {
    let locations = enabled_locations(groups);
    if locations.is_empty() {
        return None;
    }

    let current_index = current
        .and_then(|id| enabled_location_for_id(groups, id))
        .and_then(|location| {
            locations
                .iter()
                .position(|candidate| *candidate == location)
        });

    let target = match (current_index, forward) {
        (None, true) => locations.first().copied(),
        (None, false) => locations.last().copied(),
        (Some(index), true) => locations.get(index + 1).copied(),
        (Some(0), false) => None,
        (Some(index), false) => locations.get(index - 1).copied(),
    }?;

    location_id(groups, target)
}

fn group_indices_with_enabled_items(groups: &[CommandGroup]) -> Vec<usize> {
    groups
        .iter()
        .enumerate()
        .filter_map(|(group, value)| {
            value
                .items
                .iter()
                .any(|item| !item.disabled)
                .then_some(group)
        })
        .collect()
}

fn first_enabled_in_group(groups: &[CommandGroup], group: usize) -> Option<SharedString> {
    groups
        .get(group)?
        .items
        .iter()
        .find(|item| !item.disabled)
        .map(|item| item.id.clone())
}

fn last_enabled_in_group(groups: &[CommandGroup], group: usize) -> Option<SharedString> {
    groups
        .get(group)?
        .items
        .iter()
        .rev()
        .find(|item| !item.disabled)
        .map(|item| item.id.clone())
}

fn adjacent_group_id(
    groups: &[CommandGroup],
    current: Option<&SharedString>,
    forward: bool,
) -> Option<SharedString> {
    let group_indices = group_indices_with_enabled_items(groups);
    if group_indices.is_empty() {
        return None;
    }

    let current_group = current
        .and_then(|id| enabled_location_for_id(groups, id))
        .map(|location| location.group);

    let target_group = match current_group {
        None if forward => group_indices.first().copied(),
        None => group_indices.last().copied(),
        Some(current_group) if forward => group_indices
            .iter()
            .copied()
            .find(|group| *group > current_group),
        Some(current_group) => group_indices
            .iter()
            .rev()
            .copied()
            .find(|group| *group < current_group),
    }?;

    if forward {
        first_enabled_in_group(groups, target_group)
    } else {
        last_enabled_in_group(groups, target_group)
    }
}

fn activation_for_id(
    groups: &[CommandGroup],
    id: Option<&SharedString>,
) -> Option<CommandActivation> {
    let id = id?;
    let location = enabled_location_for_id(groups, id)?;
    let group = groups.get(location.group)?;
    let item = group.items.get(location.item)?;
    Some(CommandActivation::new(
        group.id.clone(),
        item.id.clone(),
        item.label.clone(),
    ))
}

fn item_element_id(
    palette_id: &ElementId,
    group_id: &SharedString,
    item_id: &SharedString,
) -> ElementId {
    let group_id = ElementId::NamedChild(Box::new(palette_id.clone()), group_id.clone());
    ElementId::NamedChild(Box::new(group_id), item_id.clone())
}

fn arrow_direction(key: &str) -> Option<bool> {
    match key {
        "down" | "arrowdown" => Some(true),
        "up" | "arrowup" => Some(false),
        _ => None,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum NavigationTarget {
    Unhandled,
    Highlight(SharedString),
    ClearHighlight,
}

fn highlight_or_clear(target: Option<SharedString>) -> NavigationTarget {
    target.map_or(
        NavigationTarget::ClearHighlight,
        NavigationTarget::Highlight,
    )
}

fn navigation_target(
    groups: &[CommandGroup],
    current: Option<&SharedString>,
    key: &str,
    modifiers: gpui::Modifiers,
) -> NavigationTarget {
    if let Some(forward) = arrow_direction(key) {
        if modifiers.platform && !modifiers.control && !modifiers.shift && !modifiers.function {
            return highlight_or_clear(if forward {
                last_enabled_id(groups)
            } else {
                first_enabled_id(groups)
            });
        }

        if modifiers.alt && !modifiers.control && !modifiers.shift && !modifiers.function {
            return highlight_or_clear(adjacent_group_id(groups, current, forward));
        }

        if !modifiers.modified() {
            return highlight_or_clear(next_or_previous_id(groups, current, forward));
        }
    }

    if !modifiers.modified() {
        match key {
            "home" | "start" => return highlight_or_clear(first_enabled_id(groups)),
            "end" => return highlight_or_clear(last_enabled_id(groups)),
            _ => {}
        }
    }

    NavigationTarget::Unhandled
}

type HighlightHandler = Rc<dyn Fn(Option<SharedString>, &mut Window, &mut App) + 'static>;
type ActivationHandler = Rc<dyn Fn(CommandActivation, &mut Window, &mut App) + 'static>;

fn emit_highlight_change(
    state: &RefCell<Option<SharedString>>,
    next: Option<SharedString>,
    handler: Option<&HighlightHandler>,
    window: &mut Window,
    cx: &mut App,
) {
    let changed = {
        let mut current = state.borrow_mut();
        if *current == next {
            false
        } else {
            current.clone_from(&next);
            true
        }
    };

    if changed && let Some(handler) = handler {
        handler(next, window, cx);
    }
}

// ---------------------------------------------------------------------------
// Palette element
// ---------------------------------------------------------------------------

/// A controlled native command palette/list primitive.
///
/// `groups` are rendered in the exact order supplied by the caller. Empty
/// groups are omitted from the visual tree so separators can only occur
/// between surviving groups. The caller feeds the ID received by
/// `on_highlight_change` back through [`Self::highlighted_id`].
#[derive(IntoElement)]
pub struct CommandPalette {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    query: SharedString,
    groups: Vec<CommandGroup>,
    highlighted_id: Option<SharedString>,
    on_highlight_change: Option<HighlightHandler>,
    on_activate: Option<ActivationHandler>,
    placeholder: Option<SharedString>,
    empty_label: SharedString,
    focus_visibility: FocusVisibility,
    debug_selector: Option<SharedString>,
    style_refinement: StyleRefinement,
}

impl CommandPalette {
    /// Creates a palette from a controlled query and already ordered groups.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        query: impl Into<SharedString>,
        groups: Vec<CommandGroup>,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            query: query.into(),
            groups,
            highlighted_id: None,
            on_highlight_change: None,
            on_activate: None,
            placeholder: None,
            empty_label: SharedString::from(DEFAULT_EMPTY_LABEL),
            focus_visibility: FocusVisibility::Hidden,
            debug_selector: None,
            style_refinement: StyleRefinement::default(),
        }
    }

    /// Sets the caller-controlled highlighted item ID.
    #[must_use]
    pub fn highlighted_id(mut self, id: Option<SharedString>) -> Self {
        self.highlighted_id = id;
        self
    }

    /// Convenience setter for a concrete highlighted ID.
    #[must_use]
    pub fn highlight(mut self, id: impl Into<SharedString>) -> Self {
        self.highlighted_id = Some(id.into());
        self
    }

    /// Installs the callback used when keyboard or pointer navigation chooses
    /// a different stable item ID.
    #[must_use]
    pub fn on_highlight_change(
        mut self,
        handler: impl Fn(Option<SharedString>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_highlight_change = Some(Rc::new(handler));
        self
    }

    /// Installs the callback used for one enabled pointer or keyboard
    /// activation.
    #[must_use]
    pub fn on_activate(
        mut self,
        handler: impl Fn(CommandActivation, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_activate = Some(Rc::new(handler));
        self
    }

    /// Supplies the controlled empty-query placeholder.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Supplies the empty-list label.
    #[must_use]
    pub fn empty_label(mut self, label: impl Into<SharedString>) -> Self {
        self.empty_label = label.into();
        self
    }

    /// Selects whether the supplied focus handle receives a visible focus
    /// treatment when focused.
    #[must_use]
    pub const fn focus_visibility(mut self, visibility: FocusVisibility) -> Self {
        self.focus_visibility = visibility;
        self
    }

    /// Adds a stable caller-owned debug selector to the root and its fixed
    /// `-input`, `-list`, and `-empty` branches.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the controlled query verbatim.
    #[must_use]
    pub fn query(&self) -> &str {
        self.query.as_str()
    }

    /// Returns the controlled query storage.
    #[must_use]
    pub fn query_value(&self) -> &SharedString {
        &self.query
    }

    /// Returns the caller-supplied groups in their original order.
    #[must_use]
    pub fn groups(&self) -> &[CommandGroup] {
        &self.groups
    }

    /// Returns the requested highlight before fallback resolution.
    #[must_use]
    pub fn requested_highlight(&self) -> Option<&SharedString> {
        self.highlighted_id.as_ref()
    }

    /// Returns the resolved enabled highlight ID.
    #[must_use]
    pub fn resolved_highlight(&self) -> Option<SharedString> {
        resolved_highlight_id(&self.groups, self.highlighted_id.as_ref())
    }

    /// Alias naming the stable-ID nature of [`Self::resolved_highlight`].
    #[must_use]
    pub fn resolved_highlight_id(&self) -> Option<SharedString> {
        self.resolved_highlight()
    }

    /// Returns the activation represented by the resolved enabled highlight.
    #[must_use]
    pub fn activation(&self) -> Option<CommandActivation> {
        let resolved = self.resolved_highlight();
        activation_for_id(&self.groups, resolved.as_ref())
    }

    /// Returns the supplied focus handle.
    #[must_use]
    pub const fn focus_handle(&self) -> &FocusHandle {
        &self.focus
    }

    /// Alias for [`Self::focus_handle`].
    #[must_use]
    pub const fn focus(&self) -> &FocusHandle {
        self.focus_handle()
    }

    /// Resolves the current visual recipe from the supplied theme.
    #[must_use]
    pub fn visual_style(&self) -> CommandStyle {
        CommandStyle::resolve(self.theme)
    }

    /// Returns the retained semantic metadata for this render recipe.
    #[must_use]
    pub fn semantics(&self) -> CommandSemantics {
        CommandSemantics {
            role: COMMAND_PALETTE_ROLE,
            item_count: self.groups.iter().map(|group| group.items.len()).sum(),
            highlighted_id: self.resolved_highlight(),
            focus_visibility: self.focus_visibility,
        }
    }

    /// Returns the selected focus visibility policy.
    #[must_use]
    pub const fn focus_visibility_value(&self) -> FocusVisibility {
        self.focus_visibility
    }
}

impl Styled for CommandPalette {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style_refinement
    }
}

struct CommandRenderContext<'a> {
    palette_id: &'a ElementId,
    theme: ArtisanTheme,
    style: CommandStyle,
    highlighted: Option<&'a SharedString>,
    highlighted_state: &'a Rc<RefCell<Option<SharedString>>>,
    on_highlight_change: Option<&'a HighlightHandler>,
    on_activate: Option<&'a ActivationHandler>,
}

struct CommandRenderParts {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    style: CommandStyle,
    query: SharedString,
    groups: Vec<CommandGroup>,
    highlighted_id: Option<SharedString>,
    on_highlight_change: Option<HighlightHandler>,
    on_activate: Option<ActivationHandler>,
    placeholder: Option<SharedString>,
    empty_label: SharedString,
    focus_visibility: FocusVisibility,
    debug_selector: Option<SharedString>,
    style_refinement: StyleRefinement,
}

fn render_input(
    theme: &ArtisanTheme,
    style: CommandStyle,
    query: SharedString,
    placeholder: Option<&SharedString>,
    selector: String,
) -> impl IntoElement + 'static {
    let query_is_empty = query.is_empty();
    let input_value = if query_is_empty {
        placeholder.cloned().unwrap_or_default()
    } else {
        query
    };

    let input_text = div()
        .flex_1()
        .min_w(px(0.0))
        .truncate()
        .text_size(style.item_text_size)
        .line_height(style.item_line_height)
        .text_color(if query_is_empty && placeholder.is_some() {
            style.muted_foreground
        } else {
            style.foreground
        })
        .child(input_value);

    div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .h(style.input_height)
        .px(style.input_horizontal_padding)
        .gap(style.input_gap)
        .rounded(style.outer_radius)
        .border_1()
        .border_color(style.input_border)
        .bg(style.input_background)
        .text_color(style.foreground)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(
            render_icon(IconStyle::resolve(
                *theme,
                AssetId::TABLER_SEARCH,
                IconSize::Default,
                IconTint::Muted,
            ))
            .size(style.input_icon_size)
            .opacity(style.search_icon_opacity),
        )
        .child(input_text)
        .debug_selector(move || selector.clone())
}

fn render_empty(
    style: CommandStyle,
    empty_label: SharedString,
    selector: String,
) -> impl IntoElement + 'static {
    div()
        .flex()
        .w_full()
        .items_center()
        .justify_center()
        .py(style.empty_vertical_padding)
        .text_size(style.empty_text_size)
        .text_color(style.muted_foreground)
        .debug_selector(move || selector.clone())
        .child(empty_label)
}

fn render_separator(style: CommandStyle) -> impl IntoElement + 'static {
    div()
        .w_full()
        .h(style.separator_height)
        .my(style.separator_margin)
        .bg(style.separator)
}

fn render_heading(style: CommandStyle, heading: SharedString) -> impl IntoElement + 'static {
    div()
        .w_full()
        .px(style.heading_horizontal_padding)
        .py(style.heading_vertical_padding)
        .text_size(style.heading_text_size)
        .font_weight(style.heading_weight)
        .text_color(style.muted_foreground)
        .whitespace_nowrap()
        .child(heading)
}

fn render_item(
    context: &CommandRenderContext<'_>,
    group: &CommandGroup,
    item: &CommandItem,
) -> impl IntoElement + 'static {
    let style = context.style;
    let is_highlighted = context.highlighted == Some(&item.id);
    let is_disabled = item.disabled;
    let item_id = item.id.clone();
    let group_id = group.id.clone();
    let mut row = div()
        .id(item_element_id(context.palette_id, &group_id, &item_id))
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w(px(0.0))
        .gap(style.item_gap)
        .px(style.item_horizontal_padding)
        .py(style.item_vertical_padding)
        .rounded(style.item_corner_radius)
        .text_size(style.item_text_size)
        .line_height(style.item_line_height)
        .text_color(if is_highlighted {
            style.highlight_foreground
        } else {
            style.foreground
        })
        .whitespace_nowrap();

    if is_highlighted {
        row = row.bg(style.highlight_background);
    }
    if is_disabled {
        row = row.opacity(style.disabled_opacity);
    }

    if let Some(icon) = item.icon {
        row = row.child(
            render_icon(IconStyle::resolve(
                context.theme,
                icon,
                IconSize::Default,
                IconTint::Inherit,
            ))
            .size(style.input_icon_size)
            .flex_shrink_0(),
        );
    }

    row = row.child(
        div()
            .flex_1()
            .min_w(px(0.0))
            .truncate()
            .child(item.label.clone()),
    );

    if let Some(shortcut) = &item.shortcut {
        row = row.child(
            div()
                .flex_shrink_0()
                .text_size(style.shortcut_text_size)
                .text_color(style.muted_foreground)
                .whitespace_nowrap()
                .child(shortcut.clone()),
        );
    }

    attach_item_interactions(
        row,
        context,
        group_id,
        item_id,
        item.label.clone(),
        is_disabled,
    )
}

fn attach_item_interactions(
    mut row: Stateful<Div>,
    context: &CommandRenderContext<'_>,
    group_id: SharedString,
    item_id: SharedString,
    label: SharedString,
    is_disabled: bool,
) -> Stateful<Div> {
    if !is_disabled {
        if let Some(handler) = context.on_highlight_change.cloned() {
            let state = context.highlighted_state.clone();
            let item_id_for_hover = item_id.clone();
            row = row.on_hover(move |hovered, window, cx| {
                if *hovered {
                    emit_highlight_change(
                        &state,
                        Some(item_id_for_hover.clone()),
                        Some(&handler),
                        window,
                        cx,
                    );
                }
            });
        }

        if context.on_activate.is_some() || context.on_highlight_change.is_some() {
            let state = context.highlighted_state.clone();
            let highlight_handler = context.on_highlight_change.cloned();
            let activation_handler = context.on_activate.cloned();
            let activation = CommandActivation::new(group_id, item_id.clone(), label);
            let item_id_for_click = item_id;
            row = row.on_click(move |event, window, cx| {
                if !event.standard_click() {
                    return;
                }

                emit_highlight_change(
                    &state,
                    Some(item_id_for_click.clone()),
                    highlight_handler.as_ref(),
                    window,
                    cx,
                );
                if let Some(handler) = &activation_handler {
                    handler(activation.clone(), window, cx);
                }
                cx.stop_propagation();
            });
        }
    }

    row
}

fn render_list(
    context: &CommandRenderContext<'_>,
    groups: &[CommandGroup],
    visible_groups: &[usize],
    selector: String,
) -> impl IntoElement + 'static {
    let mut list = div()
        .id(ElementId::NamedChild(
            Box::new(context.palette_id.clone()),
            SharedString::new_static("list"),
        ))
        .flex()
        .flex_col()
        .w_full()
        .min_h(px(0.0))
        .max_h(context.style.list_max_height)
        .py(context.style.list_scroll_padding)
        .overflow_y_scroll()
        .debug_selector(move || selector.clone());

    for (visible_index, group_index) in visible_groups.iter().copied().enumerate() {
        let group = &groups[group_index];
        if visible_index > 0 {
            list = list.child(render_separator(context.style));
        }

        if let Some(heading) = &group.heading {
            list = list.child(render_heading(context.style, heading.clone()));
        }

        for item in &group.items {
            list = list.child(render_item(context, group, item));
        }
    }

    list
}

fn apply_navigation_target(
    target: NavigationTarget,
    state: &RefCell<Option<SharedString>>,
    handler: Option<&HighlightHandler>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let next = match target {
        NavigationTarget::Unhandled => return false,
        NavigationTarget::Highlight(id) => Some(id),
        NavigationTarget::ClearHighlight => None,
    };

    window.prevent_default();
    emit_highlight_change(state, next, handler, window, cx);
    cx.stop_propagation();
    true
}

fn attach_keyboard_handler<E>(
    root: E,
    groups: Vec<CommandGroup>,
    highlighted_state: Rc<RefCell<Option<SharedString>>>,
    on_highlight_change: Option<HighlightHandler>,
    on_activate: Option<ActivationHandler>,
) -> E
where
    E: StatefulInteractiveElement,
{
    root.on_key_down(move |event, window, cx| {
        let current = highlighted_state.borrow().clone();
        if apply_navigation_target(
            navigation_target(
                &groups,
                current.as_ref(),
                event.keystroke.key.as_str(),
                event.keystroke.modifiers,
            ),
            &highlighted_state,
            on_highlight_change.as_ref(),
            window,
            cx,
        ) {
            return;
        }

        let key = event.keystroke.key.as_str();
        if !event.keystroke.modifiers.modified() && matches!(key, "enter" | "return" | "space") {
            window.prevent_default();
            if let Some(handler) = on_activate.as_ref() {
                let current = highlighted_state.borrow().clone();
                if let Some(activation) = activation_for_id(&groups, current.as_ref()) {
                    handler(activation, window, cx);
                }
            }
            cx.stop_propagation();
        }
    })
}

fn render_palette(parts: CommandRenderParts) -> impl IntoElement + 'static {
    let CommandRenderParts {
        id,
        focus,
        theme,
        style,
        query,
        groups,
        highlighted_id,
        on_highlight_change,
        on_activate,
        placeholder,
        empty_label,
        focus_visibility,
        debug_selector,
        style_refinement,
    } = parts;

    let highlighted = resolved_highlight_id(&groups, highlighted_id.as_ref());
    let highlighted_state = Rc::new(RefCell::new(highlighted.clone()));
    let root_selector = debug_selector.map_or_else(
        || DEFAULT_DEBUG_SELECTOR.to_owned(),
        |selector| selector.to_string(),
    );
    let input_selector = format!("{root_selector}-input");
    let list_selector = format!("{root_selector}-list");
    let empty_selector = format!("{root_selector}-empty");
    let palette_id = id.clone();
    let visible_groups = visible_group_indices(&groups);

    let mut root = div()
        .id(id)
        .flex()
        .flex_col()
        .w_full()
        .gap(style.outer_padding)
        .p(style.outer_padding)
        .rounded(style.outer_radius)
        .bg(style.background)
        .text_color(style.foreground)
        .overflow_hidden()
        .track_focus(&focus)
        .child(render_input(
            &theme,
            style,
            query,
            placeholder.as_ref(),
            input_selector,
        ));

    if visible_groups.is_empty() {
        root = root.child(render_empty(style, empty_label, empty_selector));
    } else {
        let context = CommandRenderContext {
            palette_id: &palette_id,
            theme,
            style,
            highlighted: highlighted.as_ref(),
            highlighted_state: &highlighted_state,
            on_highlight_change: on_highlight_change.as_ref(),
            on_activate: on_activate.as_ref(),
        };
        root = root.child(render_list(
            &context,
            &groups,
            &visible_groups,
            list_selector,
        ));
    }

    root = attach_keyboard_handler(
        root,
        groups,
        highlighted_state,
        on_highlight_change,
        on_activate,
    );

    if focus_visibility == FocusVisibility::Visible {
        root = root.focus(move |focused| {
            focused.shadow(vec![BoxShadow {
                color: style.focus_ring,
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: style.focus_ring_width,
            }])
        });
    }

    let root_selector_for_debug = root_selector.clone();
    root = root.debug_selector(move || root_selector_for_debug.clone());
    root.style().refine(&style_refinement);
    root
}

impl RenderOnce for CommandPalette {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let CommandPalette {
            id,
            focus,
            theme,
            query,
            groups,
            highlighted_id,
            on_highlight_change,
            on_activate,
            placeholder,
            empty_label,
            focus_visibility,
            debug_selector,
            style_refinement,
        } = self;

        render_palette(CommandRenderParts {
            id,
            focus,
            theme,
            style: CommandStyle::resolve(theme),
            query,
            groups,
            highlighted_id,
            on_highlight_change,
            on_activate,
            placeholder,
            empty_label,
            focus_visibility,
            debug_selector,
            style_refinement,
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/ui/command.rs"]
mod command_unit;
