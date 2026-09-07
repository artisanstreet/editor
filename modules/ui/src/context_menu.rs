//! Native, pointer-anchored context menus for the reached Artisan UI surface.
//!
//! The legacy context menu is a Bits UI right-press trigger composed with a
//! floating content recipe. GPUI 0.2.2 has the two primitives this port needs
//! (`anchored` and `deferred`), but it does not provide a menu widget or a DOM
//! roving-focus/typeahead controller. [`ContextMenuState`] is therefore kept
//! as a small deterministic engine and [`ContextMenu`] supplies the native
//! rendering and event boundary around it.
//!
//! The rendered recipe intentionally follows the audited source: a 192 px
//! minimum `rounded-2xl` popover with 4 px padding, the shared `shadow-2xl`
//! layer and a 5% foreground ring; 14 px rounded item rows with 12/8 px
//! horizontal/vertical padding; muted 12 px labels and shortcuts; and the
//! destructive focus treatment from the context-menu item wrapper. GPUI's
//! `anchored` element performs the final window collision handling while the
//! pure geometry helper documents and tests the same below/left/above
//! preference around a pointer anchor.
//!
//! Submenus, checkbox items, and radio groups are not silently represented as
//! ordinary actions. [`ContextMenuFutureExtension`] names those reserved API
//! seams so a later first-party extension can add their state and safe-polygon
//! behavior without changing the meaning of the supported entry enum.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Anchor, AnimationExt, AnyElement, App, Bounds, BoxShadow, ClickEvent, Context, Div, ElementId,
    FocusHandle, FontWeight, Hsla, InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, ParentElement as _, Pixels, Point, Render, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, anchored, canvas, deferred, div, point,
    px, size,
};

use crate::motion::{MotionPlan, MotionPolicy, MotionRecipe};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};

/// Stable debug selector for a mounted context-menu root.
pub const CONTEXT_MENU_SELECTOR: &str = "artisan-context-menu";

/// Stable debug selector for the right-press trigger shell.
pub const CONTEXT_MENU_TRIGGER_SELECTOR: &str = "artisan-context-menu-trigger";

/// Stable debug selector for the floating content panel.
pub const CONTEXT_MENU_CONTENT_SELECTOR: &str = "artisan-context-menu-content";

/// Stable debug selector shared by rendered item rows.
pub const CONTEXT_MENU_ITEM_SELECTOR: &str = "artisan-context-menu-item";

/// Gap from the pointer anchor used by the preferred placement.
pub const CONTEXT_MENU_GAP_PX: f32 = 4.0;

/// Minimum breathing room reserved between a menu and the viewport edge.
pub const CONTEXT_MENU_VIEWPORT_MARGIN_PX: f32 = 4.0;

/// The documented printable-prefix lifetime of the native typeahead buffer.
pub const CONTEXT_MENU_TYPEAHEAD_TIMEOUT_MILLIS: u64 = 1_000;

/// Explicitly documented extension seams that are not part of the supported
/// action/label/separator surface yet.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContextMenuFutureExtension {
    /// A nested menu with pointer grace-area and submenu focus semantics.
    Submenu,
    /// A stateful checked action with a tri-state presentation boundary.
    Checkbox,
    /// A mutually exclusive stateful action group.
    RadioGroup,
}

impl ContextMenuFutureExtension {
    /// Stable catalog of the reserved extension seams.
    pub const ALL: [Self; 3] = [Self::Submenu, Self::Checkbox, Self::RadioGroup];

    /// Human-readable API contract name for diagnostics and design docs.
    #[must_use]
    pub const fn contract(self) -> &'static str {
        match self {
            Self::Submenu => "submenu trigger/content with safe-polygon behavior",
            Self::Checkbox => "checked action with explicit toggle state",
            Self::RadioGroup => "mutually exclusive radio-group state",
        }
    }
}

/// Callback run after an item has closed the menu.
pub type ContextMenuAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// One enabled, disabled, ordinary, or destructive menu action.
pub struct ContextMenuItem {
    id: SharedString,
    label: SharedString,
    shortcut: Option<SharedString>,
    enabled: bool,
    destructive: bool,
    inset: bool,
    action: Option<ContextMenuAction>,
}

impl Clone for ContextMenuItem {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            label: self.label.clone(),
            shortcut: self.shortcut.clone(),
            enabled: self.enabled,
            destructive: self.destructive,
            inset: self.inset,
            action: self.action.clone(),
        }
    }
}

impl fmt::Debug for ContextMenuItem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextMenuItem")
            .field("id", &self.id)
            .field("label", &self.label)
            .field("shortcut", &self.shortcut)
            .field("enabled", &self.enabled)
            .field("destructive", &self.destructive)
            .field("inset", &self.inset)
            .field("has_action", &self.action.is_some())
            .finish()
    }
}

impl ContextMenuItem {
    /// Creates an item with a stable action identity and visible label.
    #[must_use]
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            enabled: true,
            destructive: false,
            inset: false,
            action: None,
        }
    }

    /// Creates an item whose identity is its visible label.
    #[must_use]
    pub fn from_label(label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self::new(label.clone(), label)
    }

    /// Sets the trailing keyboard shortcut presentation.
    #[must_use]
    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Sets whether the action can be highlighted or activated.
    #[must_use]
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Convenience inverse of [`Self::enabled`].
    #[must_use]
    pub fn disabled(self, disabled: bool) -> Self {
        self.enabled(!disabled)
    }

    /// Marks this action with the audited destructive treatment.
    #[must_use]
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Applies the legacy inset start padding used by grouped menu rows.
    #[must_use]
    pub fn inset(mut self, inset: bool) -> Self {
        self.inset = inset;
        self
    }

    /// Sets the action invoked after the menu closes.
    #[must_use]
    pub fn on_activate(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.action = Some(Rc::new(action));
        self
    }

    /// Alias for [`Self::on_activate`].
    #[must_use]
    pub fn on_click(self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_activate(action)
    }

    /// The stable identity used by the selection callback.
    #[must_use]
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// The visible label.
    #[must_use]
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// The optional displayed shortcut.
    #[must_use]
    pub fn shortcut_text(&self) -> Option<&SharedString> {
        self.shortcut.as_ref()
    }

    /// Whether this action accepts pointer and keyboard activation.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Whether this action uses the destructive color treatment.
    #[must_use]
    pub const fn is_destructive(&self) -> bool {
        self.destructive
    }

    /// Whether this action uses the legacy inset start padding.
    #[must_use]
    pub const fn is_inset(&self) -> bool {
        self.inset
    }

    /// The callback, if one was supplied.
    #[must_use]
    pub fn action(&self) -> Option<&ContextMenuAction> {
        self.action.as_ref()
    }
}

/// A logical group of ordinary items with an optional heading.
#[derive(Clone, Debug, Default)]
pub struct ContextMenuGroup {
    heading: Option<SharedString>,
    items: Vec<ContextMenuItem>,
}

impl ContextMenuGroup {
    /// Creates a group with a visible group heading.
    #[must_use]
    pub fn new(heading: impl Into<SharedString>) -> Self {
        Self {
            heading: Some(heading.into()),
            items: Vec::new(),
        }
    }

    /// Creates an unheaded group while retaining group ownership in the
    /// composition model.
    #[must_use]
    pub fn without_heading() -> Self {
        Self::default()
    }

    /// Appends an item to this group.
    #[must_use]
    pub fn item(mut self, item: ContextMenuItem) -> Self {
        self.items.push(item);
        self
    }

    /// The optional visible heading.
    #[must_use]
    pub fn heading(&self) -> Option<&SharedString> {
        self.heading.as_ref()
    }

    /// The group's items in display order.
    #[must_use]
    pub fn items(&self) -> &[ContextMenuItem] {
        &self.items
    }
}

/// Supported non-action content in a context menu.
#[derive(Clone, Debug)]
pub enum ContextMenuEntry {
    /// One selectable action row.
    Item(ContextMenuItem),
    /// A muted standalone label row.
    Label(SharedString),
    /// A heading plus one or more selectable action rows.
    Group(ContextMenuGroup),
    /// A one-pixel visual divider.
    Separator,
}

impl From<ContextMenuItem> for ContextMenuEntry {
    fn from(item: ContextMenuItem) -> Self {
        Self::Item(item)
    }
}

impl From<ContextMenuGroup> for ContextMenuEntry {
    fn from(group: ContextMenuGroup) -> Self {
        Self::Group(group)
    }
}

impl ContextMenuEntry {
    /// Creates a standalone label entry.
    #[must_use]
    pub fn label(label: impl Into<SharedString>) -> Self {
        Self::Label(label.into())
    }

    /// Creates a divider entry.
    #[must_use]
    pub const fn separator() -> Self {
        Self::Separator
    }
}

/// Deterministic open/highlight/typeahead state for a context menu.
///
/// The engine has no window, clock, or callback dependency. Callers provide
/// the pointer anchor, enabled flags, labels, and millisecond timestamps so
/// state transitions can be tested without a renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuState {
    open: bool,
    anchor: Option<Point<Pixels>>,
    highlighted: Option<usize>,
    enabled: Vec<bool>,
    typeahead: String,
    last_typeahead_millis: Option<u64>,
    generation: u64,
}

impl Default for ContextMenuState {
    fn default() -> Self {
        Self::new(0)
    }
}

impl ContextMenuState {
    /// Creates a closed state with `item_count` enabled selectable slots.
    #[must_use]
    pub fn new(item_count: usize) -> Self {
        Self {
            open: false,
            anchor: None,
            highlighted: None,
            enabled: vec![true; item_count],
            typeahead: String::new(),
            last_typeahead_millis: None,
            generation: 0,
        }
    }

    /// Replaces enabled flags while preserving a still-valid highlight.
    pub fn set_enabled(&mut self, enabled: &[bool]) {
        if self.enabled.as_slice() == enabled {
            return;
        }
        self.enabled = enabled.to_vec();
        if self.open && self.highlighted.is_none_or(|index| !self.is_enabled(index)) {
            self.highlighted = self.first_enabled();
        }
    }

    /// Opens at a pointer anchor using the current enabled flags.
    pub fn open(&mut self, anchor: Point<Pixels>) {
        let enabled = self.enabled.clone();
        self.open_at(anchor, &enabled);
    }

    /// Opens at a pointer anchor and selects the first enabled row.
    pub fn open_at(&mut self, anchor: Point<Pixels>, enabled: &[bool]) {
        self.set_enabled(enabled);
        self.open = true;
        self.anchor = Some(anchor);
        self.highlighted = self.first_enabled();
        self.clear_typeahead();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Closes and clears all transient menu state.
    pub fn close(&mut self) {
        if !self.open && self.anchor.is_none() && self.highlighted.is_none() {
            return;
        }
        self.open = false;
        self.anchor = None;
        self.highlighted = None;
        self.clear_typeahead();
        self.generation = self.generation.wrapping_add(1);
    }

    /// Whether the state is currently open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// The pointer anchor for the current open instance.
    #[must_use]
    pub fn anchor(&self) -> Option<Point<Pixels>> {
        self.anchor
    }

    /// The highlighted selectable item index.
    #[must_use]
    pub const fn highlighted_index(&self) -> Option<usize> {
        self.highlighted
    }

    /// Alias for [`Self::highlighted_index`].
    #[must_use]
    pub const fn highlighted(&self) -> Option<usize> {
        self.highlighted
    }

    /// The number of selectable items represented by the state.
    #[must_use]
    pub fn item_count(&self) -> usize {
        self.enabled.len()
    }

    /// Returns whether a selectable index is enabled.
    #[must_use]
    pub fn is_enabled(&self, index: usize) -> bool {
        self.enabled.get(index).copied().unwrap_or(false)
    }

    /// Returns the animation identity generation for this open/close stream.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Moves to the next enabled row, wrapping at the end.
    pub fn move_next(&mut self) -> Option<usize> {
        self.move_by(true)
    }

    /// Moves to the previous enabled row, wrapping at the beginning.
    pub fn move_previous(&mut self) -> Option<usize> {
        self.move_by(false)
    }

    /// Jumps to the first enabled row.
    pub fn move_first(&mut self) -> Option<usize> {
        if !self.open {
            return None;
        }
        self.highlighted = self.first_enabled();
        self.highlighted
    }

    /// Jumps to the last enabled row.
    pub fn move_last(&mut self) -> Option<usize> {
        if !self.open {
            return None;
        }
        self.highlighted = self.enabled.iter().rposition(|enabled| *enabled);
        self.highlighted
    }

    /// Adds one printable character to the one-second prefix typeahead.
    ///
    /// A repeated single character cycles through matching rows. A different
    /// character extends the prefix; when that extension misses, the buffer
    /// restarts at the new character and searches again.
    pub fn handle_typeahead<L: AsRef<str>>(
        &mut self,
        input: char,
        now_millis: u64,
        labels: &[L],
    ) -> Option<usize> {
        if !self.open || input.is_control() || labels.len() != self.enabled.len() {
            return None;
        }

        if self.last_typeahead_millis.is_some_and(|last| {
            now_millis < last
                || now_millis.saturating_sub(last) >= CONTEXT_MENU_TYPEAHEAD_TIMEOUT_MILLIS
        }) {
            self.clear_typeahead();
        }

        let normalized_input = input.to_lowercase().collect::<String>();
        let repeated_single_character =
            self.typeahead.chars().count() == 1 && self.typeahead == normalized_input;

        let (next_buffer, matched) = if repeated_single_character {
            (
                normalized_input.clone(),
                self.find_prefix_after_highlight(&normalized_input, labels),
            )
        } else {
            let mut extended = self.typeahead.clone();
            extended.push_str(&normalized_input);
            if let Some(index) = self.find_prefix_after_highlight(&extended, labels) {
                (extended, Some(index))
            } else {
                (
                    normalized_input.clone(),
                    self.find_prefix_after_highlight(&normalized_input, labels),
                )
            }
        };

        self.typeahead = next_buffer;
        self.last_typeahead_millis = Some(now_millis);
        if let Some(index) = matched {
            self.highlighted = Some(index);
        }
        matched
    }

    /// Returns the current raw typeahead buffer.
    #[must_use]
    pub fn typeahead_buffer(&self) -> &str {
        &self.typeahead
    }

    /// Activates the highlighted enabled item and closes the state.
    pub fn activate_highlighted(&mut self) -> Option<usize> {
        let index = self.highlighted?;
        self.activate(index).then_some(index)
    }

    /// Activates one enabled item and closes the state.
    pub fn activate(&mut self, index: usize) -> bool {
        if !self.open || !self.is_enabled(index) {
            return false;
        }
        self.close();
        true
    }

    fn move_by(&mut self, forward: bool) -> Option<usize> {
        if !self.open || self.enabled.is_empty() {
            return None;
        }

        let count = self.enabled.len();
        let mut index = match self.highlighted {
            Some(current) if forward => (current + 1) % count,
            Some(0) => count - 1,
            Some(current) => current - 1,
            None if forward => 0,
            None => count - 1,
        };

        for _ in 0..count {
            if self.enabled[index] {
                self.highlighted = Some(index);
                return self.highlighted;
            }
            index = if forward {
                (index + 1) % count
            } else if index == 0 {
                count - 1
            } else {
                index - 1
            };
        }
        None
    }

    fn first_enabled(&self) -> Option<usize> {
        self.enabled.iter().position(|enabled| *enabled)
    }

    fn find_prefix_after_highlight<L: AsRef<str>>(
        &self,
        prefix: &str,
        labels: &[L],
    ) -> Option<usize> {
        let count = self.enabled.len();
        if count == 0 || labels.len() != count {
            return None;
        }

        let start = self.highlighted.map_or(0, |index| (index + 1) % count);

        for offset in 0..count {
            let index = (start + offset) % count;
            if self.enabled[index] && labels[index].as_ref().to_lowercase().starts_with(prefix) {
                return Some(index);
            }
        }
        None
    }

    fn clear_typeahead(&mut self) {
        self.typeahead.clear();
        self.last_typeahead_millis = None;
    }
}

/// Preferred placement around a pointer anchor after collision resolution.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ContextMenuPlacement {
    /// Below and to the right of the pointer.
    #[default]
    BelowRight,
    /// Below and to the left of the pointer.
    BelowLeft,
    /// Above and to the right of the pointer.
    AboveRight,
    /// Above and to the left of the pointer.
    AboveLeft,
}

impl ContextMenuPlacement {
    /// Whether the menu hangs to the left of the anchor.
    #[must_use]
    pub const fn is_left(self) -> bool {
        matches!(self, Self::BelowLeft | Self::AboveLeft)
    }

    /// Whether the menu hangs above the anchor.
    #[must_use]
    pub const fn is_above(self) -> bool {
        matches!(self, Self::AboveRight | Self::AboveLeft)
    }
}

/// Placed menu bounds with the collision decision that produced them.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextMenuGeometry {
    /// The chosen placement around the anchor.
    pub placement: ContextMenuPlacement,
    /// The resolved menu bounds in viewport pixels.
    pub bounds: Bounds<Pixels>,
}

impl ContextMenuGeometry {
    /// Whether `point` falls inside the resolved bounds.
    #[must_use]
    pub fn contains(&self, point: Point<Pixels>) -> bool {
        self.bounds.contains(&point)
    }
}

/// Resolves menu bounds preferring below-right, flipping each axis on overflow.
#[must_use]
pub fn context_menu_geometry(
    anchor: Point<Pixels>,
    menu: Size<Pixels>,
    viewport: Size<Pixels>,
    gap: Pixels,
    margin: Pixels,
) -> ContextMenuGeometry {
    let fits_right = anchor.x + gap + menu.width <= viewport.width - margin;
    let fits_below = anchor.y + gap + menu.height <= viewport.height - margin;
    let (placement, origin) = match (fits_below, fits_right) {
        (true, true) => (
            ContextMenuPlacement::BelowRight,
            point(anchor.x + gap, anchor.y + gap),
        ),
        (true, false) => (
            ContextMenuPlacement::BelowLeft,
            point(anchor.x - gap - menu.width, anchor.y + gap),
        ),
        (false, true) => (
            ContextMenuPlacement::AboveRight,
            point(anchor.x + gap, anchor.y - gap - menu.height),
        ),
        (false, false) => (
            ContextMenuPlacement::AboveLeft,
            point(anchor.x - gap - menu.width, anchor.y - gap - menu.height),
        ),
    };
    ContextMenuGeometry {
        placement,
        bounds: Bounds { origin, size: menu },
    }
}

/// Resolves geometry, additionally clamping oversized menus into the margins.
#[must_use]
pub fn resolve_context_menu_geometry(
    anchor: Point<Pixels>,
    menu: Size<Pixels>,
    viewport: Size<Pixels>,
    gap: Pixels,
    margin: Pixels,
) -> ContextMenuGeometry {
    let available = size(
        px((f32::from(viewport.width) - 2.0 * f32::from(margin)).max(0.0)),
        px((f32::from(viewport.height) - 2.0 * f32::from(margin)).max(0.0)),
    );
    let clamped = size(
        px(f32::from(menu.width).min(f32::from(available.width))),
        px(f32::from(menu.height).min(f32::from(available.height))),
    );
    let mut geometry = context_menu_geometry(anchor, clamped, viewport, gap, margin);
    if geometry.bounds.origin.x < margin {
        geometry.bounds.origin.x = margin;
    }
    if geometry.bounds.origin.y < margin {
        geometry.bounds.origin.y = margin;
    }
    geometry
}

/// Theme-resolved geometry and paint for a context menu.
#[derive(Clone, Debug)]
pub struct ContextMenuStyle {
    /// Viewport margin consumed by the collision clamp.
    pub viewport_margin: Pixels,
    /// Popover minimum width.
    pub menu_min_width: Pixels,
    /// Popover inner padding.
    pub menu_padding: Pixels,
    /// Popover corner radius.
    pub menu_corner_radius: Pixels,
    /// Popover background.
    pub menu_background: Hsla,
    /// Popover foreground.
    pub menu_foreground: Hsla,
    /// Shared control text size.
    pub item_text_size: Pixels,
    /// Shared control line height.
    pub item_line_height: Pixels,
    /// Popover shadow layer.
    pub menu_shadow: BoxShadow,
    /// Popover hairline ring.
    pub menu_ring: BoxShadow,
    /// Resolved motion for this open instance.
    pub motion: MotionPlan,
    /// Pointer anchor gap.
    pub anchor_gap: Pixels,
    /// Gap between a check indicator and label.
    pub item_gap: Pixels,
    /// Item corner radius.
    pub item_corner_radius: Pixels,
    /// Item horizontal padding.
    pub item_horizontal_padding: Pixels,
    /// Item vertical padding.
    pub item_vertical_padding: Pixels,
    /// Resting item foreground.
    pub item_foreground: Hsla,
    /// Highlighted item background.
    pub item_hover_background: Hsla,
    /// Highlighted item foreground.
    pub item_hover_foreground: Hsla,
    /// Resting destructive foreground.
    pub destructive_foreground: Hsla,
    /// Destructive highlighted background.
    pub destructive_hover_background: Hsla,
    /// Destructive highlighted foreground.
    pub destructive_hover_foreground: Hsla,
    /// Disabled item opacity.
    pub disabled_opacity: f32,
    /// Shortcut foreground.
    pub shortcut_foreground: Hsla,
    /// Highlighted shortcut foreground.
    pub shortcut_active_foreground: Hsla,
    /// Label horizontal padding.
    pub label_horizontal_padding: Pixels,
    /// Label vertical padding.
    pub label_vertical_padding: Pixels,
    /// Label text size.
    pub label_text_size: Pixels,
    /// Label line height.
    pub label_line_height: Pixels,
    /// Label foreground.
    pub label_foreground: Hsla,
    /// Group heading horizontal padding.
    pub group_heading_horizontal_padding: Pixels,
    /// Group heading vertical padding.
    pub group_heading_vertical_padding: Pixels,
    /// Group heading foreground.
    pub group_heading_foreground: Hsla,
    /// Separator height.
    pub separator_height: Pixels,
    /// Separator paint.
    pub separator_color: Hsla,
}

impl ContextMenuStyle {
    /// Resolves the menu recipe from the shared theme and motion policy.
    #[must_use]
    pub fn resolve_with_motion(theme: ArtisanTheme, policy: MotionPolicy) -> Self {
        let destructive_alpha = match theme.mode {
            ThemeMode::Light => 0.10,
            ThemeMode::Dark => 0.20,
        };
        Self {
            viewport_margin: px(4.0),
            menu_min_width: px(192.0),
            menu_padding: px(4.0),
            menu_corner_radius: RadiusTokens::value(RadiusStep::X2l),
            menu_background: theme.colors.popover.to_paint(),
            menu_foreground: theme.colors.popover_foreground.to_paint(),
            item_text_size: theme.typography.control_text,
            item_line_height: px(f32::from(theme.typography.control_text) * 1.43),
            menu_shadow: theme.elevation.menu_shadow[0].to_box_shadow(),
            menu_ring: BoxShadow {
                color: theme.colors.foreground.with_alpha(0.05).to_paint(),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(0.0),
                spread_radius: px(1.0),
                inset: false,
            },
            motion: policy.resolve(MotionRecipe::MenuOpen),
            anchor_gap: px(4.0),
            item_gap: px(10.0),
            item_corner_radius: RadiusTokens::value(RadiusStep::Xl),
            item_horizontal_padding: px(12.0),
            item_vertical_padding: px(8.0),
            item_foreground: theme.colors.popover_foreground.to_paint(),
            item_hover_background: theme.colors.accent.to_paint(),
            item_hover_foreground: theme.colors.accent_foreground.to_paint(),
            destructive_foreground: theme.colors.destructive.to_paint(),
            destructive_hover_background: theme
                .colors
                .destructive
                .with_alpha(destructive_alpha)
                .to_paint(),
            destructive_hover_foreground: theme.colors.destructive.to_paint(),
            disabled_opacity: 0.5,
            shortcut_foreground: theme.colors.muted_foreground.to_paint(),
            shortcut_active_foreground: theme.colors.accent_foreground.to_paint(),
            label_horizontal_padding: px(12.0),
            label_vertical_padding: px(4.0),
            label_text_size: theme.typography.label_text,
            label_line_height: px(f32::from(theme.typography.label_text) * 1.5),
            label_foreground: theme.colors.muted_foreground.to_paint(),
            group_heading_horizontal_padding: px(12.0),
            group_heading_vertical_padding: px(4.0),
            group_heading_foreground: theme.colors.muted_foreground.to_paint(),
            separator_height: px(1.0),
            separator_color: theme.colors.border.with_alpha(0.5).to_paint(),
        }
    }

    /// Returns the legacy inset start padding used by grouped menu rows.
    #[must_use]
    pub fn item_inset_padding(&self) -> Pixels {
        let _ = self;
        px(24.0)
    }

    /// Returns the focus ring painted around the open menu.
    #[must_use]
    pub fn focus_ring(&self) -> BoxShadow {
        self.menu_ring.clone()
    }

    /// Returns the resting shadow stack painted under the menu.
    #[must_use]
    pub fn menu_shadows(&self) -> Vec<BoxShadow> {
        vec![self.menu_shadow.clone()]
    }
}

/// The open/close motion transition for one menu instance.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContextMenuTransition {
    /// The menu is opening.
    Open,
    /// The menu is closing.
    Close,
}

impl ContextMenuTransition {
    /// Resolves this transition into a motion plan under the policy.
    #[must_use]
    pub fn plan(self, policy: MotionPolicy) -> MotionPlan {
        match self {
            Self::Open => policy.resolve(MotionRecipe::MenuOpen),
            Self::Close => policy.resolve(MotionRecipe::MenuClose),
        }
    }
}

/// The animated open/close phase for one menu instance.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ContextMenuPhase {
    /// No menu is presented.
    Closed,
    /// The open animation is running.
    Opening,
    /// The menu is fully presented.
    Open,
    /// The close animation is running.
    Closing,
}

impl ContextMenuPhase {
    /// Advances the phase for a render with `open` intent and `ready` content.
    #[must_use]
    pub fn transition(self, open: bool, ready: bool, policy: MotionPolicy) -> Self {
        match self {
            Self::Closed => {
                if open && ready {
                    match policy {
                        MotionPolicy::Reduced => Self::Open,
                        MotionPolicy::Full => Self::Opening,
                    }
                } else {
                    Self::Closed
                }
            }
            Self::Opening => {
                if open && ready {
                    Self::Open
                } else if open {
                    Self::Opening
                } else {
                    Self::Closing
                }
            }
            Self::Open => {
                if open {
                    Self::Open
                } else {
                    Self::Closing
                }
            }
            Self::Closing => {
                if open || ready {
                    Self::Closing
                } else {
                    Self::Closed
                }
            }
        }
    }

    /// Whether menu content is presented in this phase.
    #[must_use]
    pub const fn content_present(self) -> bool {
        !matches!(self, Self::Closed)
    }
}

/// Builder for the caller-rendered trigger element.
type ContextMenuTriggerBuilder = Rc<dyn Fn() -> Div>;

/// A pointer-anchored native context menu view.
pub struct ContextMenu {
    id: ElementId,
    trigger_focus: FocusHandle,
    menu_focus: FocusHandle,
    theme: ArtisanTheme,
    motion_policy: MotionPolicy,
    trigger: ContextMenuTriggerBuilder,
    entries: Vec<ContextMenuEntry>,
    state: ContextMenuState,
    menu_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    root_debug_selector: SharedString,
    last_activation: Option<String>,
}

impl ContextMenu {
    /// Creates a menu with a caller-rendered trigger and no entries.
    pub fn new(id: impl Into<ElementId>, theme: ArtisanTheme, cx: &mut Context<Self>) -> Self {
        Self {
            id: id.into(),
            trigger_focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            theme,
            motion_policy: MotionPolicy::Full,
            trigger: Rc::new(div),
            entries: Vec::new(),
            state: ContextMenuState::new(0),
            menu_bounds: Rc::new(RefCell::new(None)),
            root_debug_selector: SharedString::from(String::new()),
            last_activation: None,
        }
    }

    /// Selects the motion policy for this menu instance.
    #[must_use]
    pub fn motion_policy(mut self, policy: MotionPolicy) -> Self {
        self.motion_policy = policy;
        self
    }

    /// Renders the trigger through the supplied builder.
    #[must_use]
    pub fn trigger(mut self, builder: impl Fn() -> Div + 'static) -> Self {
        self.trigger = Rc::new(builder);
        self
    }

    /// Appends one selectable item.
    #[must_use]
    pub fn item(mut self, item: ContextMenuItem) -> Self {
        self.entries.push(ContextMenuEntry::Item(item));
        self
    }

    /// Appends one non-selectable label row.
    #[must_use]
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.entries.push(ContextMenuEntry::label(label.into()));
        self
    }

    /// Appends one visual separator.
    #[must_use]
    pub fn separator(mut self) -> Self {
        self.entries.push(ContextMenuEntry::separator());
        self
    }

    /// Appends one logical group.
    #[must_use]
    pub fn group(mut self, group: ContextMenuGroup) -> Self {
        self.entries.push(ContextMenuEntry::Group(group));
        self
    }

    /// Adds a stable debug selector for the menu root.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.root_debug_selector = selector.into();
        self
    }

    /// Returns the deterministic interaction state.
    #[must_use]
    pub fn state(&self) -> &ContextMenuState {
        &self.state
    }

    /// Returns the stable identity of the last activation, if any.
    #[must_use]
    pub fn last_activation(&self) -> Option<String> {
        self.last_activation.clone()
    }

    /// Returns the trigger focus handle.
    #[must_use]
    pub fn trigger_focus(&self) -> &FocusHandle {
        &self.trigger_focus
    }

    /// Returns the enabled flags for every selectable item in render order.
    fn selectable_flags(&self) -> Vec<bool> {
        let mut flags = Vec::new();
        for entry in &self.entries {
            match entry {
                ContextMenuEntry::Item(item) => flags.push(item.is_enabled()),
                ContextMenuEntry::Group(group) => {
                    flags.extend(group.items().iter().map(ContextMenuItem::is_enabled));
                }
                ContextMenuEntry::Label(_) | ContextMenuEntry::Separator => {}
            }
        }
        flags
    }

    /// Returns the selectable item at a render-order index.
    fn item_at(&self, index: usize) -> Option<&ContextMenuItem> {
        let mut cursor = 0;
        for entry in &self.entries {
            match entry {
                ContextMenuEntry::Item(item) => {
                    if cursor == index {
                        return Some(item);
                    }
                    cursor += 1;
                }
                ContextMenuEntry::Group(group) => {
                    for item in group.items() {
                        if cursor == index {
                            return Some(item);
                        }
                        cursor += 1;
                    }
                }
                ContextMenuEntry::Label(_) | ContextMenuEntry::Separator => {}
            }
        }
        None
    }

    /// Refreshes enabled flags before an open transition.
    fn refresh_enabled(&mut self) {
        let enabled = self.selectable_flags();
        self.state.set_enabled(&enabled);
    }

    fn content_id(&self) -> ElementId {
        ElementId::NamedChild(Arc::new(self.id.clone()), "content".into())
    }

    fn trigger_id(&self) -> ElementId {
        ElementId::NamedChild(Arc::new(self.id.clone()), "trigger".into())
    }

    fn item_id(&self, index: usize) -> ElementId {
        ElementId::NamedChild(Arc::new(self.content_id()), format!("item-{index}").into())
    }

    /// Commits one enabled item, records the activation, and closes the menu.
    fn commit(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.item_at(index) else {
            return;
        };
        if !item.is_enabled() {
            return;
        }
        let action = item.action().cloned();
        let id = String::from(item.id().clone());
        self.state.close();
        self.last_activation = Some(id);
        window.focus(&self.trigger_focus, cx);
        if let Some(action) = action {
            action(window, cx);
        }
        cx.notify();
    }

    fn handle_item_click(
        &mut self,
        index: usize,
        _event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit(index, window, cx);
    }

    fn handle_right_press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let enabled = self.selectable_flags();
        self.refresh_enabled();
        self.state.open_at(event.position, &enabled);
        window.focus(&self.menu_focus, cx);
        cx.notify();
    }

    fn handle_trigger_left_press(
        &mut self,
        _event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_open() {
            self.state.close();
            window.focus(&self.trigger_focus, cx);
            cx.notify();
        }
    }

    fn handle_outside_press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_open() {
            return;
        }
        if self
            .menu_bounds
            .borrow()
            .is_some_and(|bounds| bounds.contains(&event.position))
        {
            return;
        }
        self.state.close();
        window.focus(&self.trigger_focus, cx);
        cx.notify();
    }

    fn handle_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let pressed = event.keystroke.key.as_str().to_ascii_lowercase();
        match pressed.as_str() {
            "escape" | "esc" => {
                self.state.close();
                window.focus(&self.trigger_focus, cx);
                cx.notify();
            }
            "arrowdown" | "down" => {
                self.state.move_next();
                cx.notify();
            }
            "arrowup" | "up" => {
                self.state.move_previous();
                cx.notify();
            }
            "home" => {
                self.state.move_first();
                cx.notify();
            }
            "end" => {
                self.state.move_last();
                cx.notify();
            }
            "enter" | "return" | "space" => {
                if let Some(index) = self.state.highlighted_index() {
                    self.commit(index, window, cx);
                }
            }
            _ => {}
        }
    }

    /// Renders one item row with destructive/highlight treatment.
    fn render_item(
        &self,
        item: &ContextMenuItem,
        index: usize,
        style: &ContextMenuStyle,
        cx: &Context<Self>,
    ) -> AnyElement {
        let highlighted = self.state.highlighted_index() == Some(index);
        let normal_foreground = if item.is_destructive() {
            style.destructive_foreground
        } else {
            style.item_foreground
        };
        let active_background = if item.is_destructive() {
            style.destructive_hover_background
        } else {
            style.item_hover_background
        };
        let active_foreground = if item.is_destructive() {
            style.destructive_hover_foreground
        } else {
            style.item_hover_foreground
        };

        let mut row = div()
            .id(self.item_id(index))
            .w_full()
            .min_w(px(0.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(style.item_gap)
            .rounded(style.item_corner_radius)
            .px(style.item_horizontal_padding)
            .py(style.item_vertical_padding)
            .text_size(style.item_text_size)
            .line_height(style.item_line_height)
            .text_color(normal_foreground)
            .whitespace_nowrap()
            .cursor_default()
            .debug_selector(|| CONTEXT_MENU_ITEM_SELECTOR.to_string())
            .on_click(cx.listener(move |view: &mut Self, event, window, cx| {
                view.handle_item_click(index, event, window, cx);
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .child(item.label().clone()),
            );

        if item.is_inset() {
            row = row.pl(style.item_inset_padding());
        }

        if highlighted {
            row = row.bg(active_background).text_color(active_foreground);
        }

        if item.is_enabled() {
            row = row.hover(move |hover| hover.bg(active_background).text_color(active_foreground));
        } else {
            row = row.opacity(style.disabled_opacity);
        }

        if let Some(shortcut) = item.shortcut_text() {
            let shortcut_foreground = if highlighted {
                style.shortcut_active_foreground
            } else {
                style.shortcut_foreground
            };

            row = row.child(
                div()
                    .flex_shrink_0()
                    .text_size(style.label_text_size)
                    .line_height(style.label_line_height)
                    .text_color(shortcut_foreground)
                    .whitespace_nowrap()
                    .child(shortcut.clone()),
            );
        }

        row.into_any_element()
    }

    fn render_label(label: &SharedString, style: &ContextMenuStyle) -> AnyElement {
        div()
            .w_full()
            .px(style.label_horizontal_padding)
            .py(style.label_vertical_padding)
            .text_size(style.label_text_size)
            .line_height(style.label_line_height)
            .text_color(style.label_foreground)
            .whitespace_nowrap()
            .child(label.clone())
            .into_any_element()
    }

    fn render_group_heading(heading: &SharedString, style: &ContextMenuStyle) -> AnyElement {
        div()
            .w_full()
            .px(style.group_heading_horizontal_padding)
            .py(style.group_heading_vertical_padding)
            .text_size(style.item_text_size)
            .line_height(style.item_line_height)
            .font_weight(FontWeight::from(500.0))
            .text_color(style.group_heading_foreground)
            .whitespace_nowrap()
            .child(heading.clone())
            .into_any_element()
    }

    fn render_separator(style: &ContextMenuStyle) -> AnyElement {
        div()
            .w_full()
            .h(style.separator_height)
            .mx(px(-4.0))
            .my(px(4.0))
            .bg(style.separator_color)
            .into_any_element()
    }

    fn render_entries(&self, style: &ContextMenuStyle, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut rendered = Vec::new();
        let mut item_index = 0;

        for entry in &self.entries {
            match entry {
                ContextMenuEntry::Item(item) => {
                    rendered.push(self.render_item(item, item_index, style, cx));
                    item_index += 1;
                }
                ContextMenuEntry::Label(label) => rendered.push(Self::render_label(label, style)),
                ContextMenuEntry::Group(group) => {
                    if let Some(heading) = group.heading() {
                        rendered.push(Self::render_group_heading(heading, style));
                    }

                    for item in group.items() {
                        rendered.push(self.render_item(item, item_index, style, cx));
                        item_index += 1;
                    }
                }
                ContextMenuEntry::Separator => rendered.push(Self::render_separator(style)),
            }
        }

        rendered
    }

    fn render_menu(&self, viewport: Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        let anchor = self.state.anchor()?;
        let style = ContextMenuStyle::resolve_with_motion(self.theme, self.motion_policy);

        let available_width =
            px((f32::from(viewport.width) - 2.0 * f32::from(style.viewport_margin)).max(0.0));
        let available_height =
            px((f32::from(viewport.height) - 2.0 * f32::from(style.viewport_margin)).max(0.0));
        let min_width = px(f32::from(style.menu_min_width)
            .min(f32::from(available_width))
            .max(0.0));

        let bounds_cell = Rc::clone(&self.menu_bounds);
        let bounds_probe = canvas(
            move |_, _, _| (),
            move |bounds, (), _, _| {
                *bounds_cell.borrow_mut() = Some(bounds);
            },
        )
        .absolute()
        .size_full();

        let focus_ring = style.focus_ring();
        let menu_shadow = style.menu_shadow.clone();
        let menu_ring = style.menu_ring.clone();

        let mut panel = div()
            .id(self.content_id())
            .relative()
            .track_focus(&self.menu_focus)
            .on_key_down(cx.listener(Self::handle_menu_key))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .min_w(min_width)
            .max_w(available_width)
            .max_h(available_height)
            .p(style.menu_padding)
            .rounded(style.menu_corner_radius)
            .bg(style.menu_background)
            .text_color(style.menu_foreground)
            .text_size(style.item_text_size)
            .line_height(style.item_line_height)
            .shadow(style.menu_shadows())
            .focus(move |focused| focused.shadow(vec![menu_shadow, menu_ring, focus_ring]))
            .debug_selector(|| CONTEXT_MENU_CONTENT_SELECTOR.to_string())
            .child(bounds_probe);

        for entry in self.render_entries(&style, cx) {
            panel = panel.child(entry);
        }

        let panel = animate_menu(panel, style.motion, self.state.generation());

        Some(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(anchor)
                .offset(point(style.anchor_gap, style.anchor_gap))
                .child(panel)
                .into_any_element(),
        )
    }
}

impl Render for ContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let trigger = (self.trigger)();
        let menu = self.render_menu(window.viewport_size(), cx);
        let root_selector = self.root_debug_selector.to_string();

        let mut root = div()
            .id(self.id.clone())
            .tab_group()
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
            .debug_selector(move || root_selector)
            .child(
                div()
                    .id(self.trigger_id())
                    .track_focus(&self.trigger_focus)
                    .cursor_default()
                    .on_key_down(cx.listener(Self::handle_menu_key))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::handle_right_press))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::handle_trigger_left_press),
                    )
                    .debug_selector(|| CONTEXT_MENU_TRIGGER_SELECTOR.to_string())
                    .child(trigger),
            );

        if let Some(menu) = menu {
            root = root.child(deferred(menu));
        }

        root
    }
}

fn animate_menu(panel: Stateful<Div>, motion: MotionPlan, generation: u64) -> AnyElement {
    let Some(animation) = motion.animation() else {
        return panel.into_any_element();
    };

    let animation_id = format!("artisan-context-menu-open-{generation}");

    panel
        .opacity(0.0)
        .with_animation(
            ElementId::Name(animation_id.into()),
            animation.gpui_clock(),
            move |panel, progress| panel.opacity(smooth_out_sample(progress)),
        )
        .into_any_element()
}

fn smooth_out_sample(progress: f32) -> f32 {
    if progress <= 0.0 {
        return 0.0;
    }
    if progress >= 1.0 {
        return 1.0;
    }

    let mut lower = 0.0_f32;
    let mut upper = 1.0_f32;

    for _ in 0..48 {
        let t = lower.midpoint(upper);
        if smooth_out_axis(t, 0.22, 0.36) < progress {
            lower = t;
        } else {
            upper = t;
        }
    }

    smooth_out_axis(lower.midpoint(upper), 1.0, 1.0)
}

fn smooth_out_axis(t: f32, first: f32, second: f32) -> f32 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
}
