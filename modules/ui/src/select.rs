//! Controlled native select/listbox primitives for Artisan settings surfaces.
//!
//! SelectState is independent from GPUI. It owns highlight, controlled-open
//! reconciliation, and printable typeahead; Select turns those transitions
//! into the existing Artisan theme, icon, focus, and deferred-overlay recipes.
//!
//! GPUI does not currently expose a platform accessibility tree. The render
//! recipe therefore keeps stable role, label, and selection metadata in public
//! semantic records without pretending those records are native accessibility
//! attributes.

use std::cell::RefCell;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use artisan_assets::AssetId;
use gpui::prelude::{FluentBuilder, Refineable};
use gpui::{
    App, BoxShadow, ClickEvent, Div, ElementId, FocusHandle, FontWeight, InteractiveElement,
    IntoElement, KeyDownEvent, MouseDownEvent, ParentElement, Pixels, RenderOnce, ScrollHandle,
    SharedString, Stateful, StatefulInteractiveElement, StyleRefinement, Styled, Window, deferred,
    div, point, px, transparent_black,
};

use crate::button::FocusVisibility;
use crate::icon::{IconSize, IconStyle, IconTint, icon};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};

/// Stable selector for an unprefixed select trigger.
pub const SELECT_TRIGGER_SELECTOR: &str = "artisan-select-trigger";
/// Stable selector for an unprefixed select content layer.
pub const SELECT_CONTENT_SELECTOR: &str = "artisan-select-content";
/// Stable selector for an unprefixed select viewport.
pub const SELECT_VIEWPORT_SELECTOR: &str = "artisan-select-viewport";

/// Printable typeahead remains active for this long between keystrokes.
pub const TYPEAHEAD_TIMEOUT: Duration = Duration::from_secs(1);

const CONTENT_GAP_PX: f32 = 4.0;
const DISABLED_OPACITY: f32 = 0.5;

/// Values accepted by a select.
///
/// Hashing supplies stable selectors without exposing arbitrary caller data in
/// selector text.
pub trait SelectValue: Clone + Eq + Hash + 'static {}

impl<T> SelectValue for T where T: Clone + Eq + Hash + 'static {}

/// The two trigger sizes reached by the audited select surfaces.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SelectSize {
    /// The default h-9 / 36 px control.
    #[default]
    Default,
    /// The compact sm / h-8 / 32 px control.
    Small,
}

/// Whether the open list can reveal more content above or below its viewport.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SelectScrollState {
    /// The viewport is at both ends, or does not overflow.
    #[default]
    None,
    /// More content is available above the viewport.
    Start,
    /// More content is available below the viewport.
    End,
    /// More content is available in both directions.
    Both,
}

/// Alias describing the same two-direction scroll-edge state.
pub type SelectScrollEdges = SelectScrollState;

impl SelectScrollState {
    /// Builds edge state from the two independently observable directions.
    #[must_use]
    pub const fn from_edges(can_scroll_up: bool, can_scroll_down: bool) -> Self {
        match (can_scroll_up, can_scroll_down) {
            (false, false) => Self::None,
            (true, false) => Self::Start,
            (false, true) => Self::End,
            (true, true) => Self::Both,
        }
    }

    /// Whether the up scroll affordance should be painted.
    #[must_use]
    pub const fn can_scroll_up(self) -> bool {
        matches!(self, Self::Start | Self::Both)
    }

    /// Whether the down scroll affordance should be painted.
    #[must_use]
    pub const fn can_scroll_down(self) -> bool {
        matches!(self, Self::End | Self::Both)
    }
}

/// One selectable item with a stable value and visible label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectItem<V: SelectValue = SharedString> {
    value: V,
    label: SharedString,
    disabled: bool,
}

impl<V: SelectValue> SelectItem<V> {
    /// Creates an enabled item.
    #[must_use]
    pub fn new(value: V, label: impl Into<SharedString>) -> Self {
        Self {
            value,
            label: label.into(),
            disabled: false,
        }
    }

    /// Marks the item disabled.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the caller-owned stable value.
    #[must_use]
    pub const fn value(&self) -> &V {
        &self.value
    }

    /// Returns the visible item label.
    #[must_use]
    pub fn label(&self) -> &str {
        self.label.as_ref()
    }

    /// Returns whether this item is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// A row in a select content layer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectEntry<V: SelectValue = SharedString> {
    /// A selectable option.
    Item(SelectItem<V>),
    /// A non-selectable group heading.
    Group(SharedString),
    /// A non-selectable visual separator.
    Separator,
}

impl<V: SelectValue> SelectEntry<V> {
    /// Constructs an enabled item row.
    #[must_use]
    pub fn item(value: V, label: impl Into<SharedString>) -> Self {
        Self::Item(SelectItem::new(value, label))
    }

    /// Constructs a disabled item row.
    #[must_use]
    pub fn disabled_item(value: V, label: impl Into<SharedString>) -> Self {
        Self::Item(SelectItem::new(value, label).disabled(true))
    }

    /// Constructs a group heading row.
    #[must_use]
    pub fn group(label: impl Into<SharedString>) -> Self {
        Self::Group(label.into())
    }

    /// Alias for group.
    #[must_use]
    pub fn group_label(label: impl Into<SharedString>) -> Self {
        Self::group(label)
    }

    /// Constructs a separator row.
    #[must_use]
    pub const fn separator() -> Self {
        Self::Separator
    }

    /// Returns the item payload when this is an item row.
    #[must_use]
    pub const fn as_item(&self) -> Option<&SelectItem<V>> {
        match self {
            Self::Item(item) => Some(item),
            Self::Group(_) | Self::Separator => None,
        }
    }

    /// Returns the visible label when this row is an item or group heading.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        match self {
            Self::Item(item) => Some(item.label()),
            Self::Group(label) => Some(label.as_ref()),
            Self::Separator => None,
        }
    }

    /// Whether this row participates in selection and keyboard navigation.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Item(item) if !item.is_disabled())
    }
}

impl<V: SelectValue> From<SelectItem<V>> for SelectEntry<V> {
    fn from(item: SelectItem<V>) -> Self {
        Self::Item(item)
    }
}
/// Theme-resolved geometry and paint for a Select.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectStyle {
    /// Trigger height.
    pub trigger_height: Pixels,
    /// Trigger horizontal padding.
    pub trigger_horizontal_padding: Pixels,
    /// Gap between the label and selector glyph.
    pub trigger_gap: Pixels,
    /// Trigger pill radius.
    pub trigger_corner_radius: Pixels,
    /// Trigger resting background.
    pub trigger_background: gpui::Hsla,
    /// Trigger foreground.
    pub trigger_foreground: gpui::Hsla,
    /// Placeholder foreground.
    pub placeholder_foreground: gpui::Hsla,
    /// Trigger border.
    pub trigger_border: gpui::Hsla,
    /// Trigger focus border.
    pub focus_border: gpui::Hsla,
    /// Trigger focus ring paint.
    pub focus_ring: gpui::Hsla,
    /// Trigger focus ring spread.
    pub focus_ring_width: Pixels,
    /// Popover minimum width.
    pub content_min_width: Pixels,
    /// Popover and viewport maximum height.
    pub content_max_height: Pixels,
    /// Popover corner radius.
    pub content_corner_radius: Pixels,
    /// Popover background.
    pub content_background: gpui::Hsla,
    /// Popover foreground.
    pub content_foreground: gpui::Hsla,
    /// Popover shadow layers.
    pub content_shadow: [BoxShadow; 1],
    /// Item horizontal padding.
    pub item_horizontal_padding: Pixels,
    /// Item vertical padding.
    pub item_vertical_padding: Pixels,
    /// Gap between a check indicator and label.
    pub item_gap: Pixels,
    /// Item corner radius.
    pub item_corner_radius: Pixels,
    /// Resting item background.
    pub item_background: gpui::Hsla,
    /// Resting item foreground.
    pub item_foreground: gpui::Hsla,
    /// Highlighted item background.
    pub item_highlight_background: gpui::Hsla,
    /// Highlighted item foreground.
    pub item_highlight_foreground: gpui::Hsla,
    /// Group heading foreground.
    pub group_label_foreground: gpui::Hsla,
    /// Group heading text size.
    pub group_label_text_size: Pixels,
    /// Separator paint.
    pub separator_color: gpui::Hsla,
    /// Scroll-edge button height.
    pub scroll_button_height: Pixels,
    /// Shared control text size.
    pub text_size: Pixels,
    /// Disabled item opacity.
    pub disabled_opacity: f32,
}

impl SelectStyle {
    /// Resolves the select recipe from the shared theme and requested size.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, size: SelectSize) -> Self {
        let trigger_height = match size {
            SelectSize::Default => theme.density.control_default,
            SelectSize::Small => theme.density.control_sm,
        };
        let trigger_background = match theme.mode {
            ThemeMode::Light => theme.surfaces.value(SurfaceStep::S100),
            ThemeMode::Dark => theme.surfaces.value(SurfaceStep::S900),
        }
        .to_paint();

        Self {
            trigger_height,
            trigger_horizontal_padding: px(12.0),
            trigger_gap: theme.spacing.steps(1.5),
            trigger_corner_radius: RadiusTokens::value(RadiusStep::X4l),
            trigger_background,
            trigger_foreground: theme.colors.foreground.to_paint(),
            placeholder_foreground: theme.colors.muted_foreground.to_paint(),
            trigger_border: theme.colors.input.to_paint(),
            focus_border: theme.colors.ring.to_paint(),
            focus_ring: theme.interaction.focus_ring_color.to_paint(),
            focus_ring_width: theme.interaction.focus_ring_width,
            content_min_width: theme.spacing.steps(36.0),
            content_max_height: theme.density.command_list_max_height,
            content_corner_radius: RadiusTokens::value(RadiusStep::X2l),
            content_background: theme.colors.popover.to_paint(),
            content_foreground: theme.colors.popover_foreground.to_paint(),
            content_shadow: theme
                .elevation
                .menu_shadow
                .map(|layer| layer.to_box_shadow()),
            item_horizontal_padding: theme.spacing.steps(3.0),
            item_vertical_padding: theme.spacing.steps(2.0),
            item_gap: theme.spacing.steps(2.5),
            item_corner_radius: RadiusTokens::value(RadiusStep::Xl),
            item_background: transparent_black(),
            item_foreground: theme.colors.popover_foreground.to_paint(),
            item_highlight_background: theme.colors.accent.to_paint(),
            item_highlight_foreground: theme.colors.accent_foreground.to_paint(),
            group_label_foreground: theme.colors.muted_foreground.to_paint(),
            group_label_text_size: theme.typography.label_text,
            separator_color: theme.colors.border.with_alpha(0.5).to_paint(),
            scroll_button_height: theme.spacing.steps(6.0),
            text_size: theme.typography.control_text,
            disabled_opacity: DISABLED_OPACITY,
        }
    }

    /// Returns the resolved trigger height.
    #[must_use]
    pub const fn height(&self) -> Pixels {
        self.trigger_height
    }
}
/// A key understood by the select state machine.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SelectKey {
    /// Enter or Space activation.
    Activate,
    /// Escape dismissal.
    Escape,
    /// Move to the next enabled item.
    ArrowDown,
    /// Move to the previous enabled item.
    ArrowUp,
    /// Reach the first enabled item.
    Home,
    /// Reach the last enabled item.
    End,
    /// Reach the first enabled item.
    PageUp,
    /// Reach the last enabled item.
    PageDown,
    /// Append a printable typeahead character.
    Character(char),
}

impl SelectKey {
    /// Parses GPUI normalized key names.
    #[must_use]
    pub fn from_key_name(key: &str) -> Option<Self> {
        match key.to_ascii_lowercase().as_str() {
            "enter" | "return" | "space" => Some(Self::Activate),
            "escape" | "esc" => Some(Self::Escape),
            "arrowdown" | "down" => Some(Self::ArrowDown),
            "arrowup" | "up" => Some(Self::ArrowUp),
            "home" => Some(Self::Home),
            "end" => Some(Self::End),
            "pageup" | "page-up" => Some(Self::PageUp),
            "pagedown" | "page-down" => Some(Self::PageDown),
            _ => None,
        }
    }
}

/// State transition produced by SelectState.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SelectAction {
    /// The key or click had no effect.
    None,
    /// The controlled component requests an open update.
    Open,
    /// The controlled component requests a close update.
    Close,
    /// The highlight moved without changing the controlled value.
    Highlight(usize),
    /// The caller should receive this item's value.
    Commit(usize),
}

impl SelectAction {
    /// Whether this transition restores the trigger focus.
    #[must_use]
    pub const fn restores_trigger_focus(self) -> bool {
        matches!(self, Self::Close | Self::Commit(_))
    }

    /// Whether this transition did something observable.
    #[must_use]
    pub const fn is_effective(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Debug, Default)]
struct SelectTypeahead {
    query: String,
    last_input: Option<Duration>,
}

impl SelectTypeahead {
    fn clear(&mut self) {
        self.query.clear();
        self.last_input = None;
    }

    fn is_active_at(&self, now: Duration) -> bool {
        self.last_input.is_some_and(|last| {
            now.saturating_sub(last) < TYPEAHEAD_TIMEOUT && !self.query.is_empty()
        })
    }
}

/// Reusable controlled open/highlight/typeahead state for a native select.
///
/// selected_index is supplied by the controlled value owner. This state never
/// changes that value on its own: Commit is the delivery point at which the
/// owner should update its value and render again.
#[derive(Clone, Debug, Default)]
pub struct SelectState {
    open: bool,
    selected_index: Option<usize>,
    highlighted: Option<usize>,
    typeahead: SelectTypeahead,
}

impl SelectState {
    /// Creates state from controlled open/value indexes and enabled-row flags.
    #[must_use]
    pub fn new(open: bool, selected_index: Option<usize>, enabled: &[bool]) -> Self {
        let highlighted = if open {
            selected_index
                .filter(|index| enabled.get(*index).copied().unwrap_or(false))
                .or_else(|| first_enabled(enabled))
        } else {
            selected_index.filter(|index| enabled.get(*index).copied().unwrap_or(false))
        };

        Self {
            open,
            selected_index,
            highlighted,
            typeahead: SelectTypeahead::default(),
        }
    }

    /// Reconciles a fresh controlled open/value render.
    pub fn reconcile(&mut self, open: bool, selected_index: Option<usize>, enabled: &[bool]) {
        let open_changed = self.open != open;
        let selection_changed = self.selected_index != selected_index;
        self.open = open;
        self.selected_index = selected_index;

        if open_changed || selection_changed {
            self.typeahead.clear();
            self.highlighted = if open {
                self.initial_open_highlight(enabled)
            } else {
                self.selected_highlight(enabled)
            };
        } else if open {
            if self
                .highlighted
                .is_none_or(|index| !enabled.get(index).copied().unwrap_or(false))
            {
                self.highlighted = self.initial_open_highlight(enabled);
            }
        } else {
            self.highlighted = self.selected_highlight(enabled);
        }
    }

    /// Returns whether the controlled select is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Returns the controlled selected row index.
    #[must_use]
    pub const fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    /// Returns the current roving highlight row index.
    #[must_use]
    pub const fn highlighted_index(&self) -> Option<usize> {
        self.highlighted
    }

    /// Returns the active typeahead query.
    #[must_use]
    pub fn typeahead_query(&self) -> &str {
        &self.typeahead.query
    }

    /// Whether the typeahead query is active at now.
    #[must_use]
    pub fn is_typeahead_active_at(&self, now: Duration) -> bool {
        self.typeahead.is_active_at(now)
    }
}
impl SelectState {
    /// Toggles the controlled open state.
    pub fn toggle(&mut self, enabled: &[bool]) -> SelectAction {
        if self.open {
            self.close(enabled)
        } else {
            self.open_with_direction(enabled, true)
        }
    }

    /// Handles Enter or Space semantics independent of GPUI dispatch.
    pub fn activate(&mut self, enabled: &[bool]) -> SelectAction {
        if self.open {
            if let Some(index) = self
                .highlighted
                .filter(|index| enabled.get(*index).copied().unwrap_or(false))
            {
                self.open = false;
                self.typeahead.clear();
                self.highlighted = self.selected_highlight(enabled);
                SelectAction::Commit(index)
            } else {
                SelectAction::None
            }
        } else {
            self.open_with_direction(enabled, true)
        }
    }

    /// Commits an item selected by pointer interaction.
    pub fn commit(&mut self, index: usize, enabled: &[bool]) -> SelectAction {
        if !enabled.get(index).copied().unwrap_or(false) {
            return SelectAction::None;
        }

        self.open = false;
        self.typeahead.clear();
        self.highlighted = self.selected_highlight(enabled);
        SelectAction::Commit(index)
    }

    /// Dismisses an open select.
    pub fn close(&mut self, enabled: &[bool]) -> SelectAction {
        if !self.open {
            return SelectAction::None;
        }

        self.open = false;
        self.typeahead.clear();
        self.highlighted = self.selected_highlight(enabled);
        SelectAction::Close
    }

    /// Handles one deterministic keyboard transition at the supplied clock.
    pub fn handle_key_at<V: SelectValue>(
        &mut self,
        key: SelectKey,
        entries: &[SelectEntry<V>],
        now: Duration,
    ) -> SelectAction {
        let enabled = entry_enabled_flags(entries);

        match key {
            SelectKey::Activate => self.activate(&enabled),
            SelectKey::Escape => self.close(&enabled),
            SelectKey::ArrowDown => {
                if self.open {
                    self.move_highlight(next_enabled(self.highlighted, &enabled, true))
                } else {
                    self.open_with_direction(&enabled, true)
                }
            }
            SelectKey::ArrowUp => {
                if self.open {
                    self.move_highlight(next_enabled(self.highlighted, &enabled, false))
                } else {
                    self.open_with_direction(&enabled, false)
                }
            }
            SelectKey::Home | SelectKey::PageUp => self.reach_boundary(first_enabled(&enabled)),
            SelectKey::End | SelectKey::PageDown => self.reach_boundary(last_enabled(&enabled)),
            SelectKey::Character(character) => {
                self.handle_typeahead(character, entries, &enabled, now)
            }
        }
    }

    /// Convenience spelling for handle_key_at.
    pub fn handle_key<V: SelectValue>(
        &mut self,
        key: SelectKey,
        entries: &[SelectEntry<V>],
        now: Duration,
    ) -> SelectAction {
        self.handle_key_at(key, entries, now)
    }

    fn initial_open_highlight(&self, enabled: &[bool]) -> Option<usize> {
        self.selected_highlight(enabled)
            .or_else(|| first_enabled(enabled))
    }

    fn selected_highlight(&self, enabled: &[bool]) -> Option<usize> {
        self.selected_index
            .filter(|index| enabled.get(*index).copied().unwrap_or(false))
    }

    fn open_with_direction(&mut self, enabled: &[bool], forward: bool) -> SelectAction {
        self.open = true;
        self.typeahead.clear();
        self.highlighted = self.selected_highlight(enabled).or_else(|| {
            if forward {
                first_enabled(enabled)
            } else {
                last_enabled(enabled)
            }
        });
        SelectAction::Open
    }

    fn move_highlight(&mut self, next: Option<usize>) -> SelectAction {
        let Some(next) = next else {
            return SelectAction::None;
        };

        self.typeahead.clear();
        if self.highlighted == Some(next) {
            SelectAction::None
        } else {
            self.highlighted = Some(next);
            SelectAction::Highlight(next)
        }
    }
}
impl SelectState {
    fn reach_boundary(&mut self, boundary: Option<usize>) -> SelectAction {
        let was_open = self.open;

        if !was_open {
            self.open = true;
            self.typeahead.clear();
        }

        let Some(boundary) = boundary else {
            return if was_open {
                SelectAction::None
            } else {
                SelectAction::Open
            };
        };

        self.typeahead.clear();
        let changed = self.highlighted != Some(boundary);
        self.highlighted = Some(boundary);

        if !was_open {
            SelectAction::Open
        } else if changed {
            SelectAction::Highlight(boundary)
        } else {
            SelectAction::None
        }
    }

    fn handle_typeahead<V: SelectValue>(
        &mut self,
        character: char,
        entries: &[SelectEntry<V>],
        enabled: &[bool],
        now: Duration,
    ) -> SelectAction {
        if character.is_control() {
            return SelectAction::None;
        }

        let typed = character.to_lowercase().collect::<String>();
        let expired = self
            .typeahead
            .last_input
            .is_none_or(|last| now.saturating_sub(last) >= TYPEAHEAD_TIMEOUT);
        let old_query = if expired {
            String::new()
        } else {
            self.typeahead.query.clone()
        };
        let repeated = !old_query.is_empty()
            && old_query.chars().count() == 1
            && old_query.eq_ignore_ascii_case(&typed);
        let mut query = if repeated {
            typed.clone()
        } else {
            format!("{old_query}{typed}")
        };

        let start = if repeated {
            next_enabled(self.highlighted, enabled, true).or_else(|| first_enabled(enabled))
        } else {
            self.highlighted.or_else(|| first_enabled(enabled))
        };
        let mut match_index = start.and_then(|start| find_prefix(entries, enabled, &query, start));

        if match_index.is_none() && !repeated && !old_query.is_empty() {
            query = typed;
            let restart = self
                .highlighted
                .or_else(|| first_enabled(enabled))
                .unwrap_or(0);
            match_index = find_prefix(entries, enabled, &query, restart);
        }

        self.typeahead.query = query;
        self.typeahead.last_input = Some(now);

        let Some(index) = match_index else {
            return SelectAction::None;
        };

        self.highlighted = Some(index);
        if self.open {
            SelectAction::Highlight(index)
        } else {
            SelectAction::Commit(index)
        }
    }
}

fn first_enabled(enabled: &[bool]) -> Option<usize> {
    enabled.iter().position(|enabled| *enabled)
}

fn last_enabled(enabled: &[bool]) -> Option<usize> {
    enabled.iter().rposition(|enabled| *enabled)
}

fn next_enabled(current: Option<usize>, enabled: &[bool], forward: bool) -> Option<usize> {
    if enabled.is_empty() || !enabled.iter().any(|enabled| *enabled) {
        return None;
    }

    let len = enabled.len();
    let current = current
        .map(|index| index % len)
        .unwrap_or_else(|| if forward { len - 1 } else { 0 });

    for step in 1..=len {
        let index = if forward {
            (current + step) % len
        } else {
            (current + len - (step % len)) % len
        };

        if enabled[index] {
            return Some(index);
        }
    }

    None
}

fn entry_enabled_flags<V: SelectValue>(entries: &[SelectEntry<V>]) -> Vec<bool> {
    entries.iter().map(SelectEntry::is_enabled).collect()
}

fn find_prefix<V: SelectValue>(
    entries: &[SelectEntry<V>],
    enabled: &[bool],
    query: &str,
    start: usize,
) -> Option<usize> {
    if entries.is_empty() || query.is_empty() {
        return None;
    }

    let start = start.min(entries.len() - 1);

    for offset in 0..entries.len() {
        let index = (start + offset) % entries.len();

        if enabled.get(index).copied().unwrap_or(false)
            && entries[index]
                .label()
                .is_some_and(|label| label.to_lowercase().starts_with(query))
        {
            return Some(index);
        }
    }

    None
}
/// Returns a deterministic value-derived selector suffix using local FNV-1a.
#[must_use]
pub fn stable_value_selector_suffix<V: Hash + ?Sized>(value: &V) -> String {
    let mut hasher = SelectFnv1aHasher(0xcbf2_9ce4_8422_2325);
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Builds an item selector below a select debug-selector prefix.
#[must_use]
pub fn item_debug_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    format!(
        "{root_selector}-item-{}",
        stable_value_selector_suffix(value)
    )
}

/// A stable semantic snapshot for the trigger and listbox.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectSemanticState {
    /// The honest role intended for the trigger seam.
    pub trigger_role: &'static str,
    /// The honest role intended for the open content seam.
    pub listbox_role: &'static str,
    /// The honest role intended for each selectable item.
    pub option_role: &'static str,
    /// Whether the controlled trigger is expanded.
    pub expanded: bool,
    /// Whether the whole control is disabled.
    pub disabled: bool,
    /// Visible trigger text.
    pub label: SharedString,
    /// Current selected visible label.
    pub value_label: Option<SharedString>,
    /// Whether the trigger currently presents its placeholder.
    pub placeholder: bool,
    /// Stable trigger selector.
    pub trigger_selector: SharedString,
    /// Stable content selector.
    pub content_selector: SharedString,
}

/// Semantic snapshot for one selectable item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectItemSemanticState {
    /// The honest role intended for this row.
    pub role: &'static str,
    /// Visible item label.
    pub label: SharedString,
    /// Whether the controlled value identifies this row.
    pub selected: bool,
    /// Whether the interaction state highlights this row.
    pub highlighted: bool,
    /// Whether this row is disabled.
    pub disabled: bool,
    /// Stable value-derived selector.
    pub selector: SharedString,
}

/// A controlled native GPUI select/listbox.
#[derive(IntoElement)]
pub struct Select<V: SelectValue = SharedString> {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    selected: Option<V>,
    open: bool,
    placeholder: SharedString,
    entries: Vec<SelectEntry<V>>,
    size: SelectSize,
    disabled: bool,
    focus_visibility: FocusVisibility,
    scroll_state: SelectScrollState,
    scroll_handle: ScrollHandle,
    on_change: Option<SelectChangeHandler<V>>,
    on_open_change: Option<SelectOpenChangeHandler>,
    debug_selector: Option<SharedString>,
    interaction_state: Option<Rc<RefCell<SelectState>>>,
    root: Div,
}

type SelectChangeHandler<V> = Rc<dyn Fn(V, &ClickEvent, &mut Window, &mut App)>;
type SelectOpenChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

impl<V: SelectValue> Select<V> {
    /// Constructs a controlled select from an ordered collection of entries.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        selected: Option<V>,
        entries: impl IntoIterator<Item = SelectEntry<V>>,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            selected,
            open: false,
            placeholder: SharedString::from("Select an option"),
            entries: entries.into_iter().collect(),
            size: SelectSize::Default,
            disabled: false,
            focus_visibility: FocusVisibility::Visible,
            scroll_state: SelectScrollState::None,
            scroll_handle: ScrollHandle::new(),
            on_change: None,
            on_open_change: None,
            debug_selector: None,
            interaction_state: None,
            root: div(),
        }
    }

    /// Constructs a select from item values and labels.
    #[must_use]
    pub fn from_items(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        selected: Option<V>,
        items: impl IntoIterator<Item = SelectItem<V>>,
    ) -> Self {
        Self::new(
            id,
            focus,
            theme,
            selected,
            items.into_iter().map(SelectEntry::Item),
        )
    }

    /// Adds an enabled item.
    #[must_use]
    pub fn item(mut self, value: V, label: impl Into<SharedString>) -> Self {
        self.entries.push(SelectEntry::item(value, label));
        self
    }

    /// Adds an already-built item.
    #[must_use]
    pub fn with_item(mut self, item: SelectItem<V>) -> Self {
        self.entries.push(item.into());
        self
    }

    /// Adds a group heading.
    #[must_use]
    pub fn group(mut self, label: impl Into<SharedString>) -> Self {
        self.entries.push(SelectEntry::group(label));
        self
    }

    /// Adds a separator.
    #[must_use]
    pub fn separator(mut self) -> Self {
        self.entries.push(SelectEntry::separator());
        self
    }

    /// Replaces the ordered entry list.
    #[must_use]
    pub fn with_entries(mut self, entries: impl IntoIterator<Item = SelectEntry<V>>) -> Self {
        self.entries = entries.into_iter().collect();
        self
    }
    /// Sets the controlled open state.
    #[must_use]
    pub const fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Alias for open.
    #[must_use]
    pub const fn is_open_controlled(self, open: bool) -> Self {
        self.open(open)
    }

    /// Sets the trigger placeholder.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Sets the trigger size.
    #[must_use]
    pub const fn size(mut self, size: SelectSize) -> Self {
        self.size = size;
        self
    }

    /// Disables the trigger and all item interaction.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Selects whether a keyboard-visible focus ring is painted.
    #[must_use]
    pub const fn focus_visibility(mut self, visibility: FocusVisibility) -> Self {
        self.focus_visibility = visibility;
        self
    }

    /// Supplies the caller-owned scroll-edge presentation state.
    #[must_use]
    pub const fn scroll_state(mut self, state: SelectScrollState) -> Self {
        self.scroll_state = state;
        self
    }

    /// Alias for scroll_state.
    #[must_use]
    pub const fn scroll_edges(self, state: SelectScrollState) -> Self {
        self.scroll_state(state)
    }

    /// Reuses a caller-owned GPUI scroll handle.
    #[must_use]
    pub fn scroll_handle(mut self, handle: ScrollHandle) -> Self {
        self.scroll_handle = handle;
        self
    }

    /// Installs the controlled selection/change callback.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(V, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Alias for on_change.
    #[must_use]
    pub fn on_value_change(
        self,
        handler: impl Fn(V, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change(handler)
    }

    /// Installs the controlled open/close callback.
    #[must_use]
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }

    /// Adds a stable selector prefix.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Supplies retained interaction state.
    #[must_use]
    pub fn with_interaction_state(mut self, state: Rc<RefCell<SelectState>>) -> Self {
        self.interaction_state = Some(state);
        self
    }

    /// Returns the controlled value retained by this render.
    #[must_use]
    pub fn selected(&self) -> Option<&V> {
        self.selected.as_ref()
    }

    /// Alias for selected.
    #[must_use]
    pub fn selected_value(&self) -> Option<&V> {
        self.selected()
    }

    /// Returns the controlled open state retained by this render.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Returns the ordered entries retained by this render.
    #[must_use]
    pub fn entries(&self) -> &[SelectEntry<V>] {
        &self.entries
    }

    /// Returns the configured placeholder text.
    #[must_use]
    pub fn placeholder_text(&self) -> &str {
        self.placeholder.as_ref()
    }

    /// Returns the current selected label.
    #[must_use]
    pub fn current_label(&self) -> Option<&str> {
        self.selected.as_ref().and_then(|selected| {
            self.entries.iter().find_map(|entry| match entry {
                SelectEntry::Item(item) if item.value() == selected => Some(item.label()),
                SelectEntry::Item(_) | SelectEntry::Group(_) | SelectEntry::Separator => None,
            })
        })
    }

    /// Alias for current_label.
    #[must_use]
    pub fn selected_label(&self) -> Option<&str> {
        self.current_label()
    }
    /// Returns the visible trigger label.
    #[must_use]
    pub fn display_label(&self) -> &str {
        self.current_label().unwrap_or(self.placeholder_text())
    }

    /// Resolves the visual recipe for this render.
    #[must_use]
    pub fn visual_style(&self) -> SelectStyle {
        SelectStyle::resolve(self.theme, self.size)
    }

    /// Returns the caller-provided scroll-edge state.
    #[must_use]
    pub const fn scroll_state_value(&self) -> SelectScrollState {
        self.scroll_state
    }

    /// Returns the caller-owned scroll handle.
    #[must_use]
    pub const fn scroll_handle_ref(&self) -> &ScrollHandle {
        &self.scroll_handle
    }

    /// Returns the semantic trigger/listbox snapshot.
    #[must_use]
    pub fn semantic_state(&self) -> SelectSemanticState {
        let value_label = self
            .current_label()
            .map(|label| SharedString::from(label.to_owned()));

        SelectSemanticState {
            trigger_role: "combobox",
            listbox_role: "listbox",
            option_role: "option",
            expanded: self.open && !self.disabled,
            disabled: self.disabled,
            label: SharedString::from(self.display_label()),
            placeholder: value_label.is_none(),
            value_label,
            trigger_selector: SharedString::from(self.trigger_selector()),
            content_selector: SharedString::from(self.content_selector()),
        }
    }

    /// Returns semantic item records in visual item order.
    #[must_use]
    pub fn item_semantics(&self, state: &SelectState) -> Vec<SelectItemSemanticState> {
        let root = self.selector_root();

        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry {
                SelectEntry::Item(item) => Some(SelectItemSemanticState {
                    role: "option",
                    label: item.label.clone(),
                    selected: self
                        .selected
                        .as_ref()
                        .is_some_and(|selected| item.value() == selected),
                    highlighted: state.highlighted_index() == Some(index),
                    disabled: self.disabled || item.is_disabled(),
                    selector: SharedString::from(item_debug_selector(&root, item.value())),
                }),
                SelectEntry::Group(_) | SelectEntry::Separator => None,
            })
            .collect()
    }

    fn selector_root(&self) -> String {
        self.debug_selector.as_ref().map_or_else(
            || "artisan-select".to_owned(),
            |selector| selector.to_string(),
        )
    }

    fn trigger_selector(&self) -> String {
        self.debug_selector.as_ref().map_or_else(
            || SELECT_TRIGGER_SELECTOR.to_owned(),
            |selector| format!("{selector}-trigger"),
        )
    }

    fn content_selector(&self) -> String {
        self.debug_selector.as_ref().map_or_else(
            || SELECT_CONTENT_SELECTOR.to_owned(),
            |selector| format!("{selector}-content"),
        )
    }

    fn viewport_selector(&self) -> String {
        self.debug_selector.as_ref().map_or_else(
            || SELECT_VIEWPORT_SELECTOR.to_owned(),
            |selector| format!("{selector}-viewport"),
        )
    }
}

impl<V: SelectValue> Styled for Select<V> {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}
impl<V: SelectValue> RenderOnce for Select<V> {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Select {
            id,
            focus,
            theme,
            selected,
            open,
            placeholder,
            entries,
            size,
            disabled,
            focus_visibility,
            scroll_state,
            scroll_handle,
            on_change,
            on_open_change,
            debug_selector,
            interaction_state,
            root,
        } = self;

        let caller_style = root.style().clone();
        let style = SelectStyle::resolve(theme, size);
        let effective_open = open && !disabled;
        let selected_index = selected_index(&entries, selected.as_ref());
        let enabled = entry_enabled_flags(&entries)
            .into_iter()
            .map(|item_enabled| item_enabled && !disabled)
            .collect::<Vec<_>>();

        let state = interaction_state.unwrap_or_else(|| {
            Rc::new(RefCell::new(SelectState::new(
                effective_open,
                selected_index,
                &enabled,
            )))
        });
        state
            .borrow_mut()
            .reconcile(effective_open, selected_index, &enabled);

        let selector_root = debug_selector.as_ref().map_or_else(
            || "artisan-select".to_owned(),
            |selector| selector.to_string(),
        );
        let trigger_selector = debug_selector.as_ref().map_or_else(
            || SELECT_TRIGGER_SELECTOR.to_owned(),
            |selector| format!("{selector}-trigger"),
        );
        let content_selector = debug_selector.as_ref().map_or_else(
            || SELECT_CONTENT_SELECTOR.to_owned(),
            |selector| format!("{selector}-content"),
        );
        let viewport_selector = debug_selector.as_ref().map_or_else(
            || SELECT_VIEWPORT_SELECTOR.to_owned(),
            |selector| format!("{selector}-viewport"),
        );

        let selected_label = selected.as_ref().and_then(|selected| {
            entries.iter().find_map(|entry| match entry {
                SelectEntry::Item(item) if item.value() == selected => Some(item.label.clone()),
                SelectEntry::Item(_) | SelectEntry::Group(_) | SelectEntry::Separator => None,
            })
        });
        let trigger_is_placeholder = selected_label.is_none();
        let trigger_label = selected_label.unwrap_or(placeholder);

        let entries_for_keys = entries.clone();
        let state_for_keys = Rc::clone(&state);
        let focus_for_keys = focus.clone();
        let on_change_for_keys = on_change.clone();
        let on_open_for_keys = on_open_change.clone();
        let scroll_for_keys = scroll_handle.clone();
        let enabled_for_keys = enabled.clone();

        let mut trigger = div()
            .id(ElementId::NamedChild(
                Box::new(id.clone()),
                "trigger".into(),
            ))
            .track_focus(&focus)
            .tab_stop(!disabled)
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .min_w_0()
            .w_full()
            .h(style.trigger_height)
            .gap(style.trigger_gap)
            .px(style.trigger_horizontal_padding)
            .py(theme.spacing.steps(2.0))
            .rounded(style.trigger_corner_radius)
            .border_1()
            .border_color(style.trigger_border)
            .bg(style.trigger_background)
            .text_color(style.trigger_foreground)
            .text_size(style.text_size)
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when(disabled, |element| element.opacity(style.disabled_opacity))
            .debug_selector(move || trigger_selector.clone());

        let label_selector = if trigger_is_placeholder {
            format!("{selector_root}-placeholder")
        } else {
            format!("{selector_root}-value")
        };
        let label_element = div()
            .min_w_0()
            .flex_1()
            .truncate()
            .text_color(if trigger_is_placeholder {
                style.placeholder_foreground
            } else {
                style.trigger_foreground
            })
            .debug_selector(move || label_selector.clone())
            .child(trigger_label);

        trigger = trigger
            .child(label_element)
            .child(div().flex_shrink_0().child(icon(IconStyle::resolve(
                theme,
                AssetId::TABLER_SELECTOR,
                IconSize::Default,
                IconTint::Muted,
            ))));

        if focus_visibility == FocusVisibility::Visible {
            trigger = trigger.focus(move |focused| {
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
        if !disabled {
            let state_for_key_handler = Rc::clone(&state);
            let entries_for_key_handler = entries_for_keys.clone();
            let focus_for_key_handler = focus_for_keys.clone();
            let on_change_for_key_handler = on_change_for_keys.clone();
            let on_open_for_key_handler = on_open_for_keys.clone();
            let scroll_for_key_handler = scroll_for_keys.clone();
            let enabled_for_key_handler = enabled_for_keys.clone();

            trigger = trigger.on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.modifiers.control
                    || event.keystroke.modifiers.alt
                    || event.keystroke.modifiers.platform
                    || event.keystroke.modifiers.function
                {
                    return;
                }

                let now = monotonic_now();
                let key_name = event.keystroke.key.as_str();
                let active_space = key_name.eq_ignore_ascii_case("space")
                    && state_for_key_handler.borrow().is_typeahead_active_at(now);

                let key = if active_space {
                    Some(SelectKey::Character(' '))
                } else {
                    SelectKey::from_key_name(key_name).or_else(|| {
                        event
                            .keystroke
                            .key_char
                            .as_ref()
                            .and_then(|text| text.chars().next())
                            .filter(|character| !character.is_control())
                            .or_else(|| {
                                if key_name.chars().count() == 1 {
                                    key_name.chars().next()
                                } else {
                                    None
                                }
                            })
                            .map(SelectKey::Character)
                    })
                };

                let Some(key) = key else {
                    return;
                };

                if matches!(key, SelectKey::Activate) {
                    return;
                }

                let action = state_for_key_handler.borrow_mut().handle_key_at(
                    key,
                    &entries_for_key_handler,
                    now,
                );

                if active_space {
                    window.prevent_default();
                    cx.stop_propagation();

                    if action.is_effective() {
                        apply_action(
                            action,
                            &state_for_key_handler,
                            &entries_for_key_handler,
                            &on_change_for_key_handler,
                            &on_open_for_key_handler,
                            &focus_for_key_handler,
                            &scroll_for_key_handler,
                            &enabled_for_key_handler,
                            false,
                            &ClickEvent::default(),
                            window,
                            cx,
                        );
                    }
                    return;
                }

                if !action.is_effective() {
                    return;
                }

                window.prevent_default();
                cx.stop_propagation();
                apply_action(
                    action,
                    &state_for_key_handler,
                    &entries_for_key_handler,
                    &on_change_for_key_handler,
                    &on_open_for_key_handler,
                    &focus_for_key_handler,
                    &scroll_for_key_handler,
                    &enabled_for_key_handler,
                    false,
                    &ClickEvent::default(),
                    window,
                    cx,
                );
            });

            let state_for_trigger = Rc::clone(&state);
            let entries_for_trigger = entries.clone();
            let on_change_for_trigger = on_change.clone();
            let on_open_for_trigger = on_open_change.clone();
            let focus_for_trigger = focus.clone();
            let scroll_for_trigger = scroll_handle.clone();
            let enabled_for_trigger = enabled.clone();

            trigger = trigger.on_click(move |event, window, cx| {
                let was_open = state_for_trigger.borrow().is_open();
                let action = if matches!(event, ClickEvent::Keyboard(_)) {
                    state_for_trigger
                        .borrow_mut()
                        .activate(&enabled_for_trigger)
                } else {
                    state_for_trigger.borrow_mut().toggle(&enabled_for_trigger)
                };

                if !action.is_effective() {
                    return;
                }

                apply_action(
                    action,
                    &state_for_trigger,
                    &entries_for_trigger,
                    &on_change_for_trigger,
                    &on_open_for_trigger,
                    &focus_for_trigger,
                    &scroll_for_trigger,
                    &enabled_for_trigger,
                    was_open,
                    &event,
                    window,
                    cx,
                );
            });
        }

        let content_id = ElementId::NamedChild(Box::new(id.clone()), "content".into());
        let mut root = div().id(id.clone()).relative().w_full().child(trigger);

        if effective_open {
            let viewport_id =
                ElementId::NamedChild(Box::new(content_id.clone()), "viewport".into());
            let mut viewport = div()
                .id(viewport_id)
                .w_full()
                .max_h(style.content_max_height)
                .overflow_y_scroll()
                .scrollbar_width(px(0.0))
                .track_scroll(&scroll_handle)
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .debug_selector(move || viewport_selector.clone());

            for (index, entry) in entries.iter().enumerate() {
                let entry_selector = match entry {
                    SelectEntry::Item(item) => item_debug_selector(&selector_root, item.value()),
                    SelectEntry::Group(_) => format!("{selector_root}-group-{index}"),
                    SelectEntry::Separator => format!("{selector_root}-separator-{index}"),
                };

                let mut row: Stateful<Div> = match entry {
                    SelectEntry::Item(item) => render_item(
                        theme,
                        item,
                        index,
                        &content_id,
                        &selector_root,
                        selected.as_ref(),
                        state.borrow().highlighted_index(),
                        &style,
                    ),
                    SelectEntry::Group(label) => div()
                        .id(ElementId::NamedChild(
                            Box::new(content_id.clone()),
                            format!("group-{index}").into(),
                        ))
                        .w_full()
                        .px(theme.spacing.steps(2.0))
                        .py(theme.spacing.steps(1.5))
                        .text_color(style.group_label_foreground)
                        .text_size(style.group_label_text_size)
                        .debug_selector(move || entry_selector.clone())
                        .child(label.clone()),
                    SelectEntry::Separator => div()
                        .id(ElementId::NamedChild(
                            Box::new(content_id.clone()),
                            format!("separator-{index}").into(),
                        ))
                        .mx(theme.spacing.steps(-1.0))
                        .my(theme.spacing.steps(1.0))
                        .h(px(1.0))
                        .bg(style.separator_color)
                        .debug_selector(move || entry_selector.clone()),
                };

                if matches!(entry, SelectEntry::Item(_))
                    && enabled.get(index).copied().unwrap_or(false)
                {
                    let state_for_entry = Rc::clone(&state);
                    let entries_for_entry = entries.clone();
                    let on_change_for_entry = on_change.clone();
                    let on_open_for_entry = on_open_change.clone();
                    let focus_for_entry = focus.clone();
                    let scroll_for_entry = scroll_handle.clone();
                    let enabled_for_entry = enabled.clone();

                    row = row.on_click(move |event, window, cx| {
                        let was_open = state_for_entry.borrow().is_open();
                        let action = state_for_entry
                            .borrow_mut()
                            .commit(index, &enabled_for_entry);

                        if !action.is_effective() {
                            return;
                        }

                        apply_action(
                            action,
                            &state_for_entry,
                            &entries_for_entry,
                            &on_change_for_entry,
                            &on_open_for_entry,
                            &focus_for_entry,
                            &scroll_for_entry,
                            &enabled_for_entry,
                            was_open,
                            event,
                            window,
                            cx,
                        );
                    });
                }

                viewport = viewport.child(row);
            }

            let mut content = div()
                .id(content_id.clone())
                .absolute()
                .top(px(f32::from(style.trigger_height) + CONTENT_GAP_PX))
                .left(px(0.0))
                .right(px(0.0))
                .min_w(style.content_min_width)
                .max_h(style.content_max_height)
                .overflow_hidden()
                .rounded(style.content_corner_radius)
                .bg(style.content_background)
                .text_color(style.content_foreground)
                .p(theme.spacing.steps(1.0))
                .shadow(style.content_shadow.to_vec())
                .debug_selector(move || content_selector.clone());

            if scroll_state.can_scroll_up() {
                let handle = scroll_handle.clone();
                content = content.child(scroll_button(
                    theme,
                    style.scroll_button_height,
                    AssetId::TABLER_CHEVRON_UP,
                    format!("{selector_root}-scroll-up"),
                    move |window| {
                        handle.scroll_to_top_of_item(0);
                        window.refresh();
                    },
                ));
            }

            content = content.child(viewport);

            if scroll_state.can_scroll_down() {
                let handle = scroll_handle.clone();
                content = content.child(scroll_button(
                    theme,
                    style.scroll_button_height,
                    AssetId::TABLER_CHEVRON_DOWN,
                    format!("{selector_root}-scroll-down"),
                    move |window| {
                        handle.scroll_to_bottom();
                        window.refresh();
                    },
                ));
            }

            let outside_state = Rc::clone(&state);
            let outside_focus = focus.clone();
            let outside_scroll = scroll_handle.clone();
            let outside_enabled = enabled.clone();
            let outside_open = on_open_change.clone();

            root = root
                .on_mouse_down_out(move |event: &MouseDownEvent, window, cx| {
                    if outside_scroll.bounds().contains(&event.position) {
                        return;
                    }

                    let action = outside_state.borrow_mut().close(&outside_enabled);
                    if !action.is_effective() {
                        return;
                    }

                    window.focus(&outside_focus);
                    if let Some(handler) = outside_open.as_ref() {
                        handler(false, window, cx);
                    }
                    cx.stop_propagation();
                })
                .child(deferred(content).with_priority(20));
        }

        if let Some(selector) = debug_selector {
            let selector = selector.to_string();
            root = root.debug_selector(move || selector.clone());
        }

        root.style().refine(&caller_style);
        root
    }
}

fn render_item<V: SelectValue>(
    theme: ArtisanTheme,
    item: &SelectItem<V>,
    index: usize,
    content_id: &ElementId,
    selector_root: &str,
    selected: Option<&V>,
    highlighted: Option<usize>,
    style: &SelectStyle,
) -> Stateful<Div> {
    let item_selected = selected.is_some_and(|selected| item.value() == selected);
    let item_highlighted = highlighted == Some(index);
    let item_selector = item_debug_selector(selector_root, item.value());
    let item_id =
        ElementId::NamedChild(Box::new(content_id.clone()), format!("item-{index}").into());

    let mut row = div()
        .id(item_id)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_w_0()
        .gap(style.item_gap)
        .px(style.item_horizontal_padding)
        .pr(px(32.0))
        .py(style.item_vertical_padding)
        .rounded(style.item_corner_radius)
        .bg(if item_highlighted {
            style.item_highlight_background
        } else {
            style.item_background
        })
        .text_color(if item_highlighted {
            style.item_highlight_foreground
        } else {
            style.item_foreground
        })
        .text_size(style.text_size)
        .debug_selector(move || item_selector.clone())
        .child(
            div()
                .min_w_0()
                .flex_1()
                .truncate()
                .whitespace_nowrap()
                .child(item.label.clone()),
        );

    if item_selected {
        row = row.child(
            div()
                .absolute()
                .right(px(8.0))
                .top(px(8.0))
                .size(px(14.0))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(IconStyle::resolve(
                    theme,
                    AssetId::TABLER_CHECK,
                    IconSize::Compact,
                    IconTint::Inherit,
                ))),
        );
    }

    if item.is_disabled() {
        row = row.opacity(style.disabled_opacity);
    } else {
        let hover_background = style.item_highlight_background;
        let hover_foreground = style.item_highlight_foreground;
        row = row.hover(move |hovered| hovered.bg(hover_background).text_color(hover_foreground));
    }

    row
}

fn scroll_button(
    theme: ArtisanTheme,
    height: Pixels,
    asset: AssetId,
    selector: String,
    on_click: impl Fn(&mut Window) + 'static,
) -> Stateful<Div> {
    let debug_selector = selector.clone();

    div()
        .id(SharedString::from(selector))
        .flex()
        .items_center()
        .justify_center()
        .w_full()
        .h(height)
        .bg(transparent_black())
        .debug_selector(move || debug_selector.clone())
        .on_click(move |_, window, _| on_click(window))
        .child(icon(IconStyle::resolve(
            theme,
            asset,
            IconSize::Default,
            IconTint::Muted,
        )))
}

fn selected_index<V: SelectValue>(
    entries: &[SelectEntry<V>],
    selected: Option<&V>,
) -> Option<usize> {
    selected.and_then(|selected| {
        entries
            .iter()
            .position(|entry| matches!(entry, SelectEntry::Item(item) if item.value() == selected))
    })
}

fn apply_action<V: SelectValue>(
    action: SelectAction,
    state: &Rc<RefCell<SelectState>>,
    entries: &[SelectEntry<V>],
    on_change: &Option<SelectChangeHandler<V>>,
    on_open_change: &Option<SelectOpenChangeHandler>,
    focus: &FocusHandle,
    scroll_handle: &ScrollHandle,
    enabled: &[bool],
    was_open: bool,
    event: &ClickEvent,
    window: &mut Window,
    cx: &mut App,
) {
    match action {
        SelectAction::None => {}
        SelectAction::Open => {
            if let Some(index) = state.borrow().highlighted_index() {
                scroll_handle.scroll_to_item(index);
            }

            if let Some(handler) = on_open_change.as_ref() {
                handler(true, window, cx);
            }
        }
        SelectAction::Close => {
            window.focus(focus);
            if let Some(handler) = on_open_change.as_ref() {
                handler(false, window, cx);
            }
        }
        SelectAction::Highlight(index) => {
            if enabled.get(index).copied().unwrap_or(false) {
                scroll_handle.scroll_to_item(index);
                window.refresh();
            }
        }
        SelectAction::Commit(index) => {
            if was_open {
                window.focus(focus);
                if let Some(handler) = on_open_change.as_ref() {
                    handler(false, window, cx);
                }
            }

            if let Some(SelectEntry::Item(item)) = entries.get(index)
                && enabled.get(index).copied().unwrap_or(false)
                && let Some(handler) = on_change.as_ref()
            {
                handler(item.value().clone(), event, window, cx);
            }
        }
    }
}

fn monotonic_now() -> Duration {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed()
}

struct SelectFnv1aHasher(u64);

impl Hasher for SelectFnv1aHasher {
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
