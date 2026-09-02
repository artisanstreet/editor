//! Native GPUI dropdown menu with a deterministic interaction state engine.
//!
//! This packet covers the reached plain-item dropdown contract: trigger-
//! anchored collision-aware placement, controlled open state, labels,
//! separators, shortcuts, disabled/destructive rows, roving keyboard
//! navigation, and deterministic typeahead. Context menus, submenus, and
//! radio/checkbox groups remain separate extensions.

use std::cell::RefCell;
use std::rc::Rc;

use crate::theme::{
    ArtisanTheme, RadiusStep, RadiusTokens, ShadowLayer, SurfaceStep, ThemeMode,
};
use gpui::{
    AnyElement, Bounds, BoxShadow, Context, Edges, FocusHandle, Hsla, InteractiveElement as _,
    IntoElement, KeyDownEvent, ParentElement as _, Pixels, Render, SharedString, Size,
    StatefulInteractiveElement as _, Styled as _, Window, div, point, px, size, transparent_black,
};

/// Typeahead remains active for one second after the last printable key.
pub const TYPEAHEAD_BUFFER_MILLIS: u64 = 1_000;

const CONTENT_MIN_WIDTH: f32 = 192.0;
const CONTENT_PADDING: f32 = 4.0;
const ITEM_HORIZONTAL_PADDING: f32 = 12.0;
const ITEM_VERTICAL_PADDING: f32 = 8.0;
const ITEM_GAP: f32 = 10.0;
const ITEM_LINE_HEIGHT: f32 = 20.0;
const LABEL_VERTICAL_PADDING: f32 = 10.0;
const LABEL_LINE_HEIGHT: f32 = 16.0;
const SEPARATOR_HEIGHT: f32 = 1.0;
const SEPARATOR_HORIZONTAL_MARGIN: f32 = -4.0;
const SEPARATOR_VERTICAL_MARGIN: f32 = 4.0;
const VIEWPORT_MARGIN: f32 = 8.0;
const DISABLED_OPACITY: f32 = 0.5;

/// One selectable dropdown item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DropdownMenuItem {
    /// Stable action identity.
    pub id: SharedString,
    /// Visible item label.
    pub label: SharedString,
    /// Optional trailing keyboard shortcut label.
    pub shortcut: Option<SharedString>,
    /// Whether the item can be highlighted or activated.
    pub disabled: bool,
    /// Whether the item uses destructive coloring.
    pub destructive: bool,
}

impl DropdownMenuItem {
    /// Creates an enabled, non-destructive item.
    #[must_use]
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            shortcut: None,
            disabled: false,
            destructive: false,
        }
    }

    /// Adds a visible shortcut label.
    #[must_use]
    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Sets the disabled state.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the destructive presentation.
    #[must_use]
    pub fn destructive(mut self, destructive: bool) -> Self {
        self.destructive = destructive;
        self
    }

    /// Returns the stable item identity.
    #[must_use]
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible label.
    #[must_use]
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Whether the item is disabled.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Whether the item can be selected.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        !self.disabled
    }

    /// Whether the item is destructive.
    #[must_use]
    pub fn is_destructive(&self) -> bool {
        self.destructive
    }
}

/// A non-selectable label, separator, or selectable item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DropdownMenuEntry {
    /// A selectable action row.
    Item(DropdownMenuItem),
    /// A non-selectable section label.
    Label(SharedString),
    /// A non-selectable divider.
    Separator,
}

impl DropdownMenuEntry {
    /// Wraps an item entry.
    #[must_use]
    pub fn item(item: DropdownMenuItem) -> Self {
        Self::Item(item)
    }

    /// Creates a label entry.
    #[must_use]
    pub fn label(label: impl Into<SharedString>) -> Self {
        Self::Label(label.into())
    }

    /// Creates a separator entry.
    #[must_use]
    pub const fn separator() -> Self {
        Self::Separator
    }

    /// Returns the item for selectable entries.
    #[must_use]
    pub fn as_item(&self) -> Option<&DropdownMenuItem> {
        match self {
            Self::Item(item) => Some(item),
            Self::Label(_) | Self::Separator => None,
        }
    }
}

impl From<DropdownMenuItem> for DropdownMenuEntry {
    fn from(item: DropdownMenuItem) -> Self {
        Self::Item(item)
    }
}

/// An action emitted after the menu has closed.
#[derive(Clone, Debug)]
pub struct DropdownMenuAction {
    item_id: SharedString,
}

impl DropdownMenuAction {
    /// Creates an action for an item identity.
    #[must_use]
    pub fn new(item_id: impl Into<SharedString>) -> Self {
        Self {
            item_id: item_id.into(),
        }
    }

    /// Returns the activated item identity.
    #[must_use]
    pub fn item_id(&self) -> &SharedString {
        &self.item_id
    }
}

#[derive(Clone, Debug, Default)]
struct TypeaheadBuffer {
    text: String,
    last_input_ms: Option<u64>,
}

impl TypeaheadBuffer {
    fn clear(&mut self) {
        self.text.clear();
        self.last_input_ms = None;
    }
}

/// Pure deterministic interaction state for a dropdown menu.
#[derive(Clone, Debug, Default)]
pub struct DropdownMenuState {
    entries: Vec<DropdownMenuEntry>,
    open: bool,
    disabled: bool,
    highlighted: Option<usize>,
    actions: Vec<DropdownMenuAction>,
    typeahead: TypeaheadBuffer,
}

impl DropdownMenuState {
    /// Creates a closed menu state.
    #[must_use]
    pub fn new(entries: impl IntoIterator<Item = DropdownMenuEntry>) -> Self {
        Self {
            entries: entries.into_iter().collect(),
            ..Self::default()
        }
    }

    /// Creates a menu state with an explicit initial open value.
    #[must_use]
    pub fn with_open(entries: impl IntoIterator<Item = DropdownMenuEntry>, open: bool) -> Self {
        let mut state = Self::new(entries);
        state.set_open(open);
        state
    }

    /// Returns the current entries.
    #[must_use]
    pub fn entries(&self) -> &[DropdownMenuEntry] {
        &self.entries
    }

    /// Replaces entries while preserving the highlighted item identity when
    /// that item remains enabled.
    pub fn set_entries(&mut self, entries: impl IntoIterator<Item = DropdownMenuEntry>) {
        let highlighted_id = self
            .highlighted
            .and_then(|index| self.item_at(index))
            .map(|item| item.id.clone());

        self.entries = entries.into_iter().collect();

        if self.open {
            self.highlighted = highlighted_id.and_then(|id| {
                self.entries.iter().position(|entry| {
                    entry
                        .as_item()
                        .is_some_and(|item| item.is_enabled() && item.id.as_ref() == id.as_ref())
                })
            });

            if self.highlighted.is_none() {
                self.highlighted = self.first_enabled_index();
            }
        } else {
            self.clear_transient();
        }
    }

    /// Whether the menu is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether opening and activation are disabled.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Sets controlled open state.
    pub fn set_open(&mut self, open: bool) {
        if open && self.disabled {
            return;
        }

        self.open = open;

        if !open {
            self.clear_transient();
        } else if self
            .highlighted
            .is_none_or(|index| !self.is_item_enabled(index))
        {
            self.highlighted = self.first_enabled_index();
        }
    }

    /// Sets disabled state. Disabling closes the menu.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;

        if disabled {
            self.open = false;
            self.clear_transient();
        }
    }

    /// Toggles the menu from its trigger.
    ///
    /// Returns the resulting open state.
    #[must_use]
    pub fn press_trigger(&mut self) -> bool {
        if self.disabled {
            return false;
        }

        self.set_open(!self.open);
        self.open
    }

    /// Dismisses the menu and clears transient navigation state.
    ///
    /// Returns whether the menu was open.
    #[must_use]
    pub fn dismiss(&mut self) -> bool {
        let was_open = self.open;
        self.open = false;
        self.clear_transient();
        was_open
    }

    /// Returns the highlighted item index.
    #[must_use]
    pub fn highlighted_index(&self) -> Option<usize> {
        self.highlighted
    }

    /// Returns the highlighted item.
    #[must_use]
    pub fn highlighted_item(&self) -> Option<&DropdownMenuItem> {
        self.highlighted.and_then(|index| self.item_at(index))
    }

    /// Returns whether an index points to an enabled item.
    #[must_use]
    pub fn is_item_enabled(&self, index: usize) -> bool {
        self.item_at(index)
            .is_some_and(DropdownMenuItem::is_enabled)
    }

    /// Returns the number of enabled selectable items.
    #[must_use]
    pub fn selectable_item_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.as_item().is_some_and(DropdownMenuItem::is_enabled))
            .count()
    }

    /// Moves to the next enabled item with wrapping.
    #[must_use]
    pub fn move_next(&mut self) -> Option<usize> {
        if !self.open {
            return None;
        }

        let len = self.entries.len();
        if len == 0 {
            return None;
        }

        let start = self.highlighted.map_or(0, |index| (index + 1) % len);

        let next = self.find_enabled_from(start, true);
        self.highlighted = next;
        next
    }

    /// Moves to the previous enabled item with wrapping.
    #[must_use]
    pub fn move_previous(&mut self) -> Option<usize> {
        if !self.open {
            return None;
        }

        let len = self.entries.len();
        if len == 0 {
            return None;
        }

        let start = self.highlighted.map_or(0, |index| (index + len - 1) % len);

        let previous = self.find_enabled_from(start, false);
        self.highlighted = previous;
        previous
    }

    /// Moves to the first enabled item.
    #[must_use]
    pub fn move_first(&mut self) -> Option<usize> {
        if !self.open {
            return None;
        }

        let first = self.first_enabled_index();
        self.highlighted = first;
        first
    }

    /// Moves to the last enabled item.
    #[must_use]
    pub fn move_last(&mut self) -> Option<usize> {
        if !self.open {
            return None;
        }

        let last = self.last_enabled_index();
        self.highlighted = last;
        last
    }

    /// Handles one deterministic printable typeahead input.
    ///
    /// Repeated single characters cycle through matching rows. Extended
    /// prefixes accumulate until the timeout expires; a missed extension
    /// restarts the buffer with the new character.
    #[must_use]
    pub fn handle_typeahead(&mut self, key: &str, now_ms: u64) -> Option<usize> {
        if !self.open {
            return None;
        }

        let mut normalized = String::new();
        for character in key.chars() {
            if !character.is_control() && !character.is_whitespace() {
                normalized.extend(character.to_lowercase());
            }
        }

        if normalized.is_empty() {
            return self.highlighted;
        }

        let expired = self
            .typeahead
            .last_input_ms
            .is_none_or(|last| now_ms.saturating_sub(last) >= TYPEAHEAD_BUFFER_MILLIS);

        let previous = if expired {
            String::new()
        } else {
            self.typeahead.text.clone()
        };

        let repeated = normalized.chars().count() == 1 && previous.as_str() == normalized.as_str();

        let (buffer, candidate) = if repeated {
            let start = self
                .highlighted
                .map_or(0, |index| (index + 1) % self.entries.len().max(1));

            (
                normalized.clone(),
                self.find_enabled_prefix_from(&normalized, start)
                    .or_else(|| self.find_enabled_prefix_from(&normalized, 0)),
            )
        } else {
            let mut extended = previous;
            extended.push_str(&normalized);

            match self.find_prefix_from(&extended, 0) {
                Some(index) => (extended, Some(index)),
                None => (normalized.clone(), self.find_prefix_from(&normalized, 0)),
            }
        };

        self.typeahead.text = buffer;
        self.typeahead.last_input_ms = Some(now_ms);

        if let Some(index) = candidate {
            self.highlighted = Some(index);
        }

        self.highlighted
    }

    /// Returns the current typeahead buffer.
    #[must_use]
    pub fn typeahead_buffer(&self) -> &str {
        &self.typeahead.text
    }

    /// Activates the highlighted item and queues its action.
    #[must_use]
    pub fn activate_highlighted(&mut self) -> bool {
        self.highlighted
            .is_some_and(|index| self.activate_index(index))
    }

    /// Activates an enabled item by entry index.
    #[must_use]
    pub fn activate_index(&mut self, index: usize) -> bool {
        let Some(action) = self
            .item_at(index)
            .filter(|item| item.is_enabled())
            .map(|item| DropdownMenuAction::new(item.id.clone()))
        else {
            return false;
        };

        self.actions.push(action);
        self.open = false;
        self.clear_transient();
        true
    }

    /// Drains actions produced since the previous call.
    #[must_use]
    pub fn take_actions(&mut self) -> Vec<DropdownMenuAction> {
        std::mem::take(&mut self.actions)
    }

    fn clear_transient(&mut self) {
        self.highlighted = None;
        self.typeahead.clear();
    }

    fn item_at(&self, index: usize) -> Option<&DropdownMenuItem> {
        self.entries.get(index).and_then(DropdownMenuEntry::as_item)
    }

    fn first_enabled_index(&self) -> Option<usize> {
        self.entries.iter().enumerate().find_map(|(index, entry)| {
            entry
                .as_item()
                .filter(|item| item.is_enabled())
                .map(|_| index)
        })
    }

    fn last_enabled_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, entry)| {
                entry
                    .as_item()
                    .filter(|item| item.is_enabled())
                    .map(|_| index)
            })
    }

    fn find_enabled_from(&self, start: usize, forward: bool) -> Option<usize> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }

        (0..len).find_map(|offset| {
            let index = if forward {
                (start + offset) % len
            } else {
                (start + len - offset % len) % len
            };

            self.is_item_enabled(index).then_some(index)
        })
    }

    fn find_enabled_prefix_from(&self, prefix: &str, start: usize) -> Option<usize> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }

        (0..len).find_map(|offset| {
            let index = (start + offset) % len;
            let item = self.item_at(index)?;
            if !item.is_enabled() {
                return None;
            }
            item.label
                .as_ref()
                .to_lowercase()
                .starts_with(prefix)
                .then_some(index)
        })
    }

    fn find_prefix_from(&self, prefix: &str, start: usize) -> Option<usize> {
        let len = self.entries.len();
        if len == 0 {
            return None;
        }

        (0..len).find_map(|offset| {
            let index = (start + offset) % len;
            let item = self.item_at(index)?;
            item.label
                .as_ref()
                .to_lowercase()
                .starts_with(prefix)
                .then_some(index)
        })
    }
}

/// Preferred side for the anchored menu.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DropdownMenuSide {
    /// Place below the trigger unless collision handling flips it.
    #[default]
    Bottom,
    /// Place above the trigger unless collision handling flips it.
    Top,
}

/// Horizontal alignment against the trigger.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DropdownMenuAlign {
    /// Align leading edges.
    #[default]
    Start,
    /// Align trailing edges.
    End,
}

/// Geometry policy shared by the state-independent placement engine and the
/// GPUI anchored renderer.
#[derive(Clone, Debug, PartialEq)]
pub struct DropdownMenuGeometry {
    /// Preferred vertical side.
    pub side: DropdownMenuSide,
    /// Horizontal alignment.
    pub align: DropdownMenuAlign,
    /// Gap between trigger and content.
    pub side_offset: Pixels,
    /// Minimum viewport breathing room.
    pub viewport_margin: Edges<Pixels>,
}

impl Default for DropdownMenuGeometry {
    fn default() -> Self {
        Self {
            side: DropdownMenuSide::default(),
            align: DropdownMenuAlign::default(),
            side_offset: px(4.0),
            viewport_margin: Edges {
                top: px(VIEWPORT_MARGIN),
                right: px(VIEWPORT_MARGIN),
                bottom: px(VIEWPORT_MARGIN),
                left: px(VIEWPORT_MARGIN),
            },
        }
    }
}

impl DropdownMenuGeometry {
    /// Creates a placement policy.
    #[must_use]
    pub fn new(
        side: DropdownMenuSide,
        align: DropdownMenuAlign,
        side_offset: Pixels,
        viewport_margin: Edges<Pixels>,
    ) -> Self {
        Self {
            side,
            align,
            side_offset,
            viewport_margin,
        }
    }

    /// Returns horizontal space available inside viewport margins.
    #[must_use]
    pub fn available_width(&self, viewport: Size<Pixels>) -> Pixels {
        px((f32::from(viewport.width)
            - f32::from(self.viewport_margin.left)
            - f32::from(self.viewport_margin.right))
        .max(0.0))
    }

    /// Returns vertical space available inside viewport margins.
    #[must_use]
    pub fn available_height(&self, viewport: Size<Pixels>) -> Pixels {
        px((f32::from(viewport.height)
            - f32::from(self.viewport_margin.top)
            - f32::from(self.viewport_margin.bottom))
        .max(0.0))
    }

    /// Resolves a collision-aware placement without GPUI rendering.
    #[must_use]
    pub fn resolve(
        &self,
        trigger: Bounds<Pixels>,
        menu_size: Size<Pixels>,
        viewport: Size<Pixels>,
    ) -> DropdownMenuPlacement {
        let trigger_x = f32::from(trigger.origin.x);
        let trigger_y = f32::from(trigger.origin.y);
        let trigger_width = f32::from(trigger.size.width);
        let trigger_height = f32::from(trigger.size.height);
        let menu_width = f32::from(menu_size.width);
        let menu_height = f32::from(menu_size.height);

        let preferred_x = match self.align {
            DropdownMenuAlign::Start => trigger_x,
            DropdownMenuAlign::End => trigger_x + trigger_width - menu_width,
        };

        let preferred_y = match self.side {
            DropdownMenuSide::Bottom => trigger_y + trigger_height + f32::from(self.side_offset),
            DropdownMenuSide::Top => trigger_y - f32::from(self.side_offset) - menu_height,
        };

        let opposite_y = match self.side {
            DropdownMenuSide::Bottom => trigger_y - f32::from(self.side_offset) - menu_height,
            DropdownMenuSide::Top => trigger_y + trigger_height + f32::from(self.side_offset),
        };

        let left_limit = f32::from(self.viewport_margin.left);
        let right_limit =
            f32::from(viewport.width) - f32::from(self.viewport_margin.right) - menu_width;
        let top_limit = f32::from(self.viewport_margin.top);
        let bottom_limit = f32::from(viewport.height) - f32::from(self.viewport_margin.bottom);

        let preferred_fits = preferred_y >= top_limit && preferred_y + menu_height <= bottom_limit;
        let opposite_fits = opposite_y >= top_limit && opposite_y + menu_height <= bottom_limit;

        let (raw_y, flipped) = if !preferred_fits && opposite_fits {
            (opposite_y, true)
        } else {
            (preferred_y, false)
        };

        let x = preferred_x.clamp(left_limit, right_limit.max(left_limit));
        let y = raw_y.clamp(top_limit, (bottom_limit - menu_height).max(top_limit));

        DropdownMenuPlacement {
            bounds: Bounds::new(point(px(x), px(y)), menu_size),
            side: if flipped {
                match self.side {
                    DropdownMenuSide::Bottom => DropdownMenuSide::Top,
                    DropdownMenuSide::Top => DropdownMenuSide::Bottom,
                }
            } else {
                self.side
            },
            align: self.align,
            flipped,
        }
    }

}

/// Resolved menu bounds and collision outcome.
#[derive(Clone, Debug, PartialEq)]
pub struct DropdownMenuPlacement {
    /// Final menu bounds.
    pub bounds: Bounds<Pixels>,
    /// Effective side.
    pub side: DropdownMenuSide,
    /// Effective alignment.
    pub align: DropdownMenuAlign,
    /// Whether the menu flipped sides.
    pub flipped: bool,
}

impl DropdownMenuPlacement {
    /// Whether the placement stays inside the supplied margins.
    #[must_use]
    pub fn fits_within(&self, viewport: Size<Pixels>, margin: Edges<Pixels>) -> bool {
        self.bounds.origin.x >= margin.left
            && self.bounds.origin.y >= margin.top
            && self.bounds.origin.x + self.bounds.size.width <= viewport.width - margin.right
            && self.bounds.origin.y + self.bounds.size.height <= viewport.height - margin.bottom
    }
}

/// Resolved content recipe.
#[derive(Clone, Copy, Debug)]
pub struct DropdownMenuContentStyle {
    /// Minimum width, matching `min-w-48`.
    pub min_width: Pixels,
    /// Uniform content padding.
    pub padding: Pixels,
    /// Content corner radius.
    pub corner_radius: Pixels,
    /// Popover background.
    pub background: Hsla,
    /// Popover foreground.
    pub foreground: Hsla,
    /// One-pixel ring color.
    pub ring_color: Hsla,
    /// Ring width.
    pub ring_width: Pixels,
    /// Menu shadow.
    pub shadow: ShadowLayer,
    /// Content text size.
    pub text_size: Pixels,
}

impl DropdownMenuContentStyle {
    /// Resolves the content recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            min_width: px(CONTENT_MIN_WIDTH),
            padding: px(CONTENT_PADDING),
            corner_radius: RadiusTokens::value(RadiusStep::X2l),
            background: theme.colors.popover.to_paint(),
            foreground: theme.colors.popover_foreground.to_paint(),
            ring_color: theme.colors.border.to_paint(),
            ring_width: px(1.0),
            shadow: theme.elevation.menu_shadow[0],
            text_size: theme.typography.control_text,
        }
    }

    /// Converts the theme shadow token into a GPUI shadow.
    #[must_use]
    pub fn menu_shadow(self) -> BoxShadow {
        self.shadow.to_box_shadow()
    }
}
/// Resolved item recipe.
#[derive(Clone, Copy, Debug)]
pub struct DropdownMenuItemStyle {
    /// Horizontal inset for item content.
    pub horizontal_padding: Pixels,
    /// Vertical inset above and below item content.
    pub vertical_padding: Pixels,
    /// Gap between an item icon, label, and shortcut.
    pub gap: Pixels,
    /// Corner radius used for hovered and focused items.
    pub corner_radius: Pixels,
    /// Normal item foreground.
    pub foreground: Hsla,
    /// Foreground used by destructive items.
    pub destructive_foreground: Hsla,
    /// Background used while an item is hovered or keyboard-focused.
    pub focus_background: Hsla,
    /// Foreground used while an item is hovered or keyboard-focused.
    pub focus_foreground: Hsla,
    /// Background used while a destructive item is hovered or focused.
    pub destructive_focus_background: Hsla,
    /// Opacity applied to disabled rows.
    pub disabled_opacity: f32,
    /// Text size used by item labels.
    pub text_size: Pixels,
}

impl DropdownMenuItemStyle {
    /// Resolves the item recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        let destructive_alpha = if theme.mode == ThemeMode::Light {
            0.10
        } else {
            0.20
        };
        Self {
            horizontal_padding: px(ITEM_HORIZONTAL_PADDING),
            vertical_padding: px(ITEM_VERTICAL_PADDING),
            gap: px(ITEM_GAP),
            corner_radius: RadiusTokens::value(RadiusStep::Xl),
            foreground: theme.colors.popover_foreground.to_paint(),
            destructive_foreground: theme.colors.destructive.to_paint(),
            focus_background: theme.colors.accent.to_paint(),
            focus_foreground: theme.colors.accent_foreground.to_paint(),
            destructive_focus_background: theme
                .colors
                .destructive
                .with_alpha(destructive_alpha)
                .to_paint(),
            disabled_opacity: DISABLED_OPACITY,
            text_size: theme.typography.control_text,
        }
    }

    /// Returns the deterministic row height: content line plus insets.
    #[must_use]
    pub fn height(&self) -> Pixels {
        self.vertical_padding + self.vertical_padding + px(ITEM_LINE_HEIGHT)
    }
}

/// Resolved section-label recipe.
#[derive(Clone, Copy, Debug)]
pub struct DropdownMenuLabelStyle {
    /// Muted label foreground.
    pub foreground: Hsla,
    /// Text size used by section labels.
    pub text_size: Pixels,
}

impl DropdownMenuLabelStyle {
    /// Resolves the label recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            foreground: SurfaceStep::S500.oklch().to_paint(),
            text_size: theme.typography.control_text,
        }
    }
}

/// Resolved shortcut-hint recipe.
#[derive(Clone, Copy, Debug)]
pub struct DropdownMenuShortcutStyle {
    /// Muted shortcut foreground.
    pub foreground: Hsla,
    /// Text size used by shortcut hints.
    pub text_size: Pixels,
}

impl DropdownMenuShortcutStyle {
    /// Resolves the shortcut recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            foreground: theme.colors.muted_foreground.to_paint(),
            text_size: theme.typography.control_text,
        }
    }
}

/// Resolved separator recipe.
#[derive(Clone, Copy, Debug)]
pub struct DropdownMenuSeparatorStyle {
    /// Separator thickness.
    pub height: Pixels,
    /// Horizontal inset applied symmetrically around a separator.
    pub horizontal_margin: Pixels,
    /// Vertical space above and below a separator.
    pub vertical_margin: Pixels,
    /// Separator color.
    pub color: Hsla,
}

impl DropdownMenuSeparatorStyle {
    /// Resolves the separator recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            height: px(SEPARATOR_HEIGHT),
            horizontal_margin: px(SEPARATOR_HORIZONTAL_MARGIN),
            vertical_margin: px(SEPARATOR_VERTICAL_MARGIN),
            color: theme.colors.border.to_paint(),
        }
    }
}

/// Returns the current typeahead clock in whole milliseconds since the Unix
/// epoch. Rendering never invents event times; only live key handling reads
/// this clock, while deterministic tests drive the state engine directly.
fn typeahead_now_ms() -> u64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis());
    u64::try_from(millis.min(u128::from(u64::MAX))).unwrap_or(u64::MAX)
}

/// Computes the deterministic content height for one entry list.
fn dropdown_content_height(style: &DropdownMenuStyle, entries: &[DropdownMenuEntry]) -> Pixels {
    let mut height = f32::from(style.content.padding) * 2.0;
    for entry in entries {
        height += match entry {
            DropdownMenuEntry::Item(_) => f32::from(style.item.height()),
            DropdownMenuEntry::Label(_) => {
                LABEL_LINE_HEIGHT + LABEL_VERTICAL_PADDING * 2.0
            }
            DropdownMenuEntry::Separator => {
                f32::from(style.separator.height) + f32::from(style.separator.vertical_margin) * 2.0
            }
        };
    }
    px(height)
}

/// Builder for the trigger affordance, called with no arguments each render.
type DropdownTriggerBuilder = Rc<dyn Fn() -> AnyElement>;

/// A fully configured dropdown menu view.
///
/// The caller owns open state through [`DropdownMenu::open`]; rendering
/// mirrors it into the deterministic [`DropdownMenuState`] engine, which
/// owns highlight, typeahead, and activation. Item activation queues a
/// typed action on the state engine and closes the menu.
pub struct DropdownMenu {
    id: SharedString,
    trigger_focus: FocusHandle,
    theme: ArtisanTheme,
    trigger: DropdownTriggerBuilder,
    entries: Vec<DropdownMenuEntry>,
    open: bool,
    selector: Option<SharedString>,
    state: DropdownMenuState,
    applied_open: bool,
    trigger_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    item_focus: Vec<Option<FocusHandle>>,
}

impl DropdownMenu {
    /// Creates a dropdown menu with a caller-rendered trigger.
    pub fn new(
        id: impl Into<SharedString>,
        trigger_focus: FocusHandle,
        theme: ArtisanTheme,
        trigger: impl Fn() -> AnyElement + 'static,
        entries: Vec<DropdownMenuEntry>,
    ) -> Self {
        Self {
            id: id.into(),
            trigger_focus,
            theme,
            trigger: Rc::new(trigger),
            entries: entries.clone(),
            open: false,
            selector: None,
            state: DropdownMenuState::new(entries),
            applied_open: false,
            trigger_bounds: Rc::new(RefCell::new(None)),
            item_focus: Vec::new(),
        }
    }

    /// Sets the controlled open state.
    #[must_use]
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Adds a stable selector. The trigger and content derive
    /// `{selector}-trigger` and `{selector}-content`; without one, the menu
    /// id is the base instead.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    /// Returns the deterministic interaction state.
    #[must_use]
    pub fn state(&self) -> &DropdownMenuState {
        &self.state
    }

    /// Returns the deterministic interaction state mutably.
    pub fn state_mut(&mut self) -> &mut DropdownMenuState {
        &mut self.state
    }

    fn base_selector(&self) -> SharedString {
        self.selector.clone().unwrap_or_else(|| self.id.clone())
    }

    fn trigger_selector(&self) -> SharedString {
        SharedString::from(format!("{}-trigger", self.base_selector()))
    }

    fn content_selector(&self) -> SharedString {
        SharedString::from(format!("{}-content", self.base_selector()))
    }

    fn focus_highlighted(&self, window: &mut Window) {
        if let Some(index) = self.state.highlighted_index()
            && let Some(Some(handle)) = self.item_focus.get(index)
        {
            window.focus(handle);
        }
    }

    fn activate_item(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.state.activate_index(index) {
            window.focus(&self.trigger_focus);
            cx.notify();
        }
    }

    fn handle_trigger_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let handled = match key {
            "escape" => self.state.dismiss(),
            "enter" | "return" | "space" => {
                let _ = self.state.press_trigger();
                true
            }
            "down" => {
                if !self.state.is_open() {
                    self.state.set_open(true);
                }
                let _ = self.state.move_next();
                self.focus_highlighted(window);
                true
            }
            "up" => {
                if !self.state.is_open() {
                    self.state.set_open(true);
                }
                let _ = self.state.move_previous();
                self.focus_highlighted(window);
                true
            }
            _ => {
                if !event.keystroke.modifiers.modified() && key.chars().count() == 1 {
                    let _ = self.state.handle_typeahead(key, typeahead_now_ms());
                    self.focus_highlighted(window);
                    true
                } else {
                    false
                }
            }
        };
        if handled {
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn handle_item_key(
        &mut self,
        index: usize,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let handled = match key {
            "escape" => {
                let _ = self.state.dismiss();
                window.focus(&self.trigger_focus);
                true
            }
            "enter" | "return" | "space" => {
                self.activate_item(index, window, cx);
                true
            }
            "down" => {
                let _ = self.state.move_next();
                self.focus_highlighted(window);
                true
            }
            "up" => {
                let _ = self.state.move_previous();
                self.focus_highlighted(window);
                true
            }
            "home" => {
                let _ = self.state.move_first();
                self.focus_highlighted(window);
                true
            }
            "end" => {
                let _ = self.state.move_last();
                self.focus_highlighted(window);
                true
            }
            _ => {
                if !event.keystroke.modifiers.modified() && key.chars().count() == 1 {
                    let _ = self.state.handle_typeahead(key, typeahead_now_ms());
                    self.focus_highlighted(window);
                    true
                } else {
                    false
                }
            }
        };
        if handled {
            window.prevent_default();
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn render_row(
        &self,
        index: usize,
        entry: &DropdownMenuEntry,
        style: &DropdownMenuStyle,
        base: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match entry {
            DropdownMenuEntry::Label(label) => div()
                .px(style.content.padding)
                .py(px(LABEL_VERTICAL_PADDING))
                .text_color(style.label.foreground)
                .child(label.clone())
                .into_any_element(),
            DropdownMenuEntry::Separator => div()
                .mx(style.separator.horizontal_margin)
                .my(style.separator.vertical_margin)
                .h(style.separator.height)
                .bg(style.separator.color)
                .into_any_element(),
            DropdownMenuEntry::Item(item) => {
                let highlighted = self.state.highlighted_index() == Some(index);
                let foreground = if item.is_destructive() {
                    style.item.destructive_foreground
                } else {
                    style.item.foreground
                };
                let background = if highlighted {
                    if item.is_destructive() {
                        style.item.destructive_focus_background
                    } else {
                        style.item.focus_background
                    }
                } else {
                    transparent_black()
                };
                let text = if highlighted && !item.is_destructive() {
                    style.item.focus_foreground
                } else {
                    foreground
                };
                let row_id =
                    SharedString::from(format!("{base}-item-{index}"));
                let mut row = div()
                    .id(row_id)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(style.item.gap)
                    .px(style.item.horizontal_padding)
                    .h(style.item.height())
                    .rounded(style.item.corner_radius)
                    .bg(background)
                    .text_color(text)
                    .child(item.label.clone());
                if let Some(shortcut) = item.shortcut.clone() {
                    row = row.child(
                        div()
                            .text_color(style.shortcut.foreground)
                            .child(shortcut),
                    );
                }
                if item.is_disabled() {
                    return row.opacity(style.item.disabled_opacity).into_any_element();
                }
                let item_selector = format!("{base}-item-{index}");
                row.track_focus(
                    self.item_focus
                        .get(index)
                        .and_then(|handle| handle.as_ref())
                        .expect("enabled rows always retain a focus handle"),
                )
                .debug_selector(move || item_selector.clone())
                .on_click(cx.listener(move |view, _, window, cx| {
                    view.activate_item(index, window, cx);
                }))
                .on_key_down(cx.listener(move |view, event, window, cx| {
                    view.handle_item_key(index, event, window, cx);
                }))
                .into_any_element()
            }
        }
    }
}

impl Render for DropdownMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.state.entries() != self.entries.as_slice() {
            self.state.set_entries(self.entries.clone());
        }
        if self.open != self.applied_open {
            self.state.set_open(self.open);
            self.applied_open = self.open;
        }
        self.item_focus.resize_with(self.entries.len(), || None);
        for (index, entry) in self.entries.iter().enumerate() {
            if entry.as_item().is_some_and(DropdownMenuItem::is_enabled) {
                if self.item_focus[index].is_none() {
                    self.item_focus[index] = Some(cx.focus_handle());
                }
            } else {
                self.item_focus[index] = None;
            }
        }

        let style =
            DropdownMenuStyle::resolve(self.theme, DropdownMenuGeometry::default());
        let trigger_selector = self.trigger_selector();
        let content_selector = self.content_selector();
        let trigger_slot = Rc::clone(&self.trigger_bounds);
        let trigger = div()
            .id(trigger_selector.clone())
            .track_focus(&self.trigger_focus)
            .tab_index(0)
            .child((self.trigger)())
            .debug_selector(move || trigger_selector.to_string())
            .on_click(cx.listener(|view, _, _, cx| {
                let _ = view.state.press_trigger();
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::handle_trigger_key));
        let trigger = div()
            .child(trigger)
            .on_children_prepainted(move |bounds, _, _| {
                *trigger_slot.borrow_mut() = bounds.first().copied();
            });

        let mut root = div().relative().flex().flex_col().items_start().child(trigger);
        if self.state.is_open() {
            let viewport = window.bounds().size;
            let width = style.content.min_width;
            let height = dropdown_content_height(&style, &self.entries);
            let trigger_rect = self.trigger_bounds.borrow().unwrap_or_default();
            let placed = style
                .geometry
                .resolve(trigger_rect, size(width, height), viewport);
            let mut content = div()
                .absolute()
                .left(placed.bounds.origin.x)
                .top(placed.bounds.origin.y)
                .w(width)
                .h(height)
                .p(style.content.padding)
                .rounded(style.content.corner_radius)
                .bg(style.content.background)
                .text_color(style.content.foreground)
                .shadow(vec![style.content.menu_shadow()])
                .overflow_hidden()
                .debug_selector(move || content_selector.to_string());
            let base = self.base_selector().to_string();
            for (index, entry) in self.entries.iter().enumerate() {
                content = content.child(self.render_row(index, entry, &style, &base, cx));
            }
            root = root.child(content);
        }
        root
    }
}

/// Resolved dropdown-menu recipe bundle.
#[derive(Clone, Debug)]
pub struct DropdownMenuStyle {
    /// Placement policy used to resolve this style.
    pub geometry: DropdownMenuGeometry,
    /// Content container recipe.
    pub content: DropdownMenuContentStyle,
    /// Item row recipe.
    pub item: DropdownMenuItemStyle,
    /// Section-label recipe.
    pub label: DropdownMenuLabelStyle,
    /// Shortcut-hint recipe.
    pub shortcut: DropdownMenuShortcutStyle,
    /// Separator recipe.
    pub separator: DropdownMenuSeparatorStyle,
}

impl DropdownMenuStyle {
    /// Resolves the full menu recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, geometry: DropdownMenuGeometry) -> Self {
        Self {
            geometry,
            content: DropdownMenuContentStyle::resolve(theme),
            item: DropdownMenuItemStyle::resolve(theme),
            label: DropdownMenuLabelStyle::resolve(theme),
            shortcut: DropdownMenuShortcutStyle::resolve(theme),
            separator: DropdownMenuSeparatorStyle::resolve(theme),
        }
    }

    /// Returns the content width for measured content, floored at the
    /// minimum width.
    #[must_use]
    pub fn content_width(&self, content: Pixels) -> Pixels {
        let content = f32::from(content);
        let minimum = f32::from(self.content.min_width);
        px(content.max(minimum))
    }

    /// Returns the content width clamped to the viewport's available width.
    #[must_use]
    pub fn content_width_for_viewport(&self, content: Pixels, viewport: Size<Pixels>) -> Pixels {
        let width = f32::from(self.content_width(content));
        let available = f32::from(self.geometry.available_width(viewport));
        px(width.min(available).max(0.0))
    }

    /// Returns the maximum menu height inside the viewport margins.
    #[must_use]
    pub fn max_height_for_viewport(&self, viewport: Size<Pixels>) -> Pixels {
        self.geometry.available_height(viewport)
    }
}

