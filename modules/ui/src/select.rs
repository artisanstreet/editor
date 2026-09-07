//! Controlled native select/listbox primitives for Artisan settings surfaces.
//!
//! `SelectState` is independent from GPUI. It owns highlight, controlled-open
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
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use artisan_assets::AssetId;
use gpui::{
    App, BoxShadow, ClickEvent, Div, ElementId, FocusHandle, InteractiveElement, IntoElement,
    KeyDownEvent, MouseDownEvent, ParentElement, Pixels, RenderOnce, ScrollHandle, SharedString,
    Stateful, StatefulInteractiveElement, Styled, Window, deferred, div, px, transparent_black,
};

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
///
/// The state is caller-owned because the pinned GPUI scroll handle does not
/// provide a portable visibility-observer callback. It is rendered as real
/// scroll-edge controls rather than decorative metadata.
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
///
/// Groups and separators are part of the same ordered list so visual order,
/// keyboard indexes, and scroll-child indexes remain inspectable and
/// deterministic.
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
                .map(super::theme::ShadowLayer::to_box_shadow),
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

/// State transition produced by `SelectState`.
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
/// `selected_index` is supplied by the controlled value owner. This state never
/// changes that value on its own: Commit is the delivery point at which the
/// owner should update its value and render again.
#[derive(Clone, Debug, Default)]
pub struct SelectState {
    open: bool,
    selected_index: Option<usize>,
    highlighted: Option<usize>,
    typeahead: SelectTypeahead,
    reconciled: Option<(bool, Option<usize>)>,
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
            reconciled: None,
        }
    }

    /// Reconciles a fresh controlled open/value render.
    ///
    /// A changed controlled value or open state wins over a stale highlight;
    /// otherwise an in-progress keyboard highlight is retained. If an item
    /// becomes disabled or disappears, the highlight falls back to the
    /// controlled selection and then the first enabled item.
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

    fn open_with_direction(&mut self, enabled: &[bool], forward: bool) -> SelectAction {
        self.open = true;
        self.typeahead.clear();
        self.highlighted = if forward {
            self.initial_open_highlight(enabled)
        } else {
            self.selected_index
                .filter(|index| enabled.get(*index).copied().unwrap_or(false))
                .or_else(|| last_enabled(enabled))
        };
        SelectAction::Open
    }

    fn initial_open_highlight(&self, enabled: &[bool]) -> Option<usize> {
        self.selected_index
            .filter(|index| enabled.get(*index).copied().unwrap_or(false))
            .or_else(|| first_enabled(enabled))
    }

    fn selected_highlight(&self, enabled: &[bool]) -> Option<usize> {
        self.selected_index
            .filter(|index| enabled.get(*index).copied().unwrap_or(false))
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
                    match step_highlight(self.highlighted, &enabled, true) {
                        Some(index) => {
                            self.highlighted = Some(index);
                            SelectAction::Highlight(index)
                        }
                        None => SelectAction::None,
                    }
                } else {
                    self.open_with_direction(&enabled, true)
                }
            }
            SelectKey::ArrowUp => {
                if self.open {
                    match step_highlight(self.highlighted, &enabled, false) {
                        Some(index) => {
                            self.highlighted = Some(index);
                            SelectAction::Highlight(index)
                        }
                        None => SelectAction::None,
                    }
                } else {
                    self.open_with_direction(&enabled, false)
                }
            }
            SelectKey::Home | SelectKey::PageUp => {
                if self.open {
                    match first_enabled(&enabled) {
                        Some(index) => {
                            self.highlighted = Some(index);
                            SelectAction::Highlight(index)
                        }
                        None => SelectAction::None,
                    }
                } else {
                    self.open_with_direction(&enabled, true)
                }
            }
            SelectKey::End | SelectKey::PageDown => {
                if self.open {
                    match last_enabled(&enabled) {
                        Some(index) => {
                            self.highlighted = Some(index);
                            SelectAction::Highlight(index)
                        }
                        None => SelectAction::None,
                    }
                } else {
                    self.open_with_direction(&enabled, true)
                }
            }
            SelectKey::Character(character) => {
                self.type_character(character, entries, &enabled, now)
            }
        }
    }

    /// Handles one printable typeahead transition at the supplied clock.
    ///
    /// An expired buffer restarts from the typed character; otherwise the
    /// character extends the buffer, unless it repeats the single-character
    /// buffer, in which case navigation cycles past the current row. When the
    /// extended buffer matches nothing, the fresh character is retried on its
    /// own. Closed selects commit the match without opening.
    fn type_character<V: SelectValue>(
        &mut self,
        character: char,
        entries: &[SelectEntry<V>],
        enabled: &[bool],
        now: Duration,
    ) -> SelectAction {
        let typed: String = character.to_lowercase().collect();
        if typed.is_empty() {
            return SelectAction::None;
        }

        let expired = !self.typeahead.is_active_at(now);
        let repeated = !expired
            && !self.typeahead.query.is_empty()
            && self.typeahead.query.chars().count() == 1
            && self.typeahead.query.eq_ignore_ascii_case(&typed);

        let query = if expired {
            typed.clone()
        } else if repeated {
            self.typeahead.query.clone()
        } else {
            format!("{}{typed}", self.typeahead.query)
        };

        let candidate = if repeated {
            let start = self
                .highlighted
                .map(|index| (index + 1) % enabled.len().max(1));
            start.and_then(|start| find_enabled_prefix(entries, &query, start))
        } else {
            find_enabled_prefix(entries, &query, 0).or_else(|| {
                if query == typed {
                    None
                } else {
                    find_enabled_prefix(entries, &typed, 0)
                }
            })
        };

        self.typeahead.query = query;
        self.typeahead.last_input = Some(now);

        match candidate {
            Some(index) => {
                self.highlighted = Some(index);
                if self.open {
                    SelectAction::Highlight(index)
                } else {
                    SelectAction::Commit(index)
                }
            }
            None => SelectAction::None,
        }
    }
}

fn first_enabled(enabled: &[bool]) -> Option<usize> {
    enabled.iter().position(|enabled| *enabled)
}

fn last_enabled(enabled: &[bool]) -> Option<usize> {
    enabled.iter().rposition(|enabled| *enabled)
}

/// Steps the highlight one enabled row in a direction, staying at the edge
/// when no further enabled row exists. Returns `None` only when no row is
/// enabled at all.
fn step_highlight(current: Option<usize>, enabled: &[bool], forward: bool) -> Option<usize> {
    let len = enabled.len();
    if !enabled.iter().any(|enabled| *enabled) {
        return None;
    }

    let start = current.unwrap_or(if forward { 0 } else { len.saturating_sub(1) });
    let mut index = start;
    loop {
        let next = if forward {
            index.checked_add(1).filter(|next| *next < len)
        } else {
            index.checked_sub(1)
        };
        match next {
            Some(next) if enabled[next] => return Some(next),
            Some(next) => index = next,
            None => return Some(start.min(len.saturating_sub(1))),
        }
    }
}

fn entry_enabled_flags<V: SelectValue>(entries: &[SelectEntry<V>]) -> Vec<bool> {
    entries.iter().map(SelectEntry::is_enabled).collect()
}

/// Finds the first enabled row whose label starts with `prefix`
/// (case-insensitive) at or after `start`, without wrapping.
fn find_enabled_prefix<V: SelectValue>(
    entries: &[SelectEntry<V>],
    prefix: &str,
    start: usize,
) -> Option<usize> {
    entries
        .iter()
        .enumerate()
        .skip(start)
        .filter(|(_, entry)| entry.is_enabled())
        .find_map(|(index, entry)| {
            entry
                .label()
                .is_some_and(|label| label.to_lowercase().starts_with(&prefix.to_lowercase()))
                .then_some(index)
        })
}

/// Borrowed inputs for one rendered select row.
struct RenderItemArgs<'a, V: SelectValue> {
    theme: &'a ArtisanTheme,
    item: &'a SelectItem<V>,
    index: usize,
    content_id: &'a ElementId,
    selector_root: &'a str,
    selected: Option<&'a V>,
    highlighted: Option<usize>,
    style: &'a SelectStyle,
}

fn render_item<V: SelectValue>(args: &RenderItemArgs<'_, V>) -> Stateful<Div> {
    let &RenderItemArgs {
        theme,
        item,
        index,
        content_id,
        selector_root,
        selected,
        highlighted,
        style,
    } = args;
    let item_selected = selected.is_some_and(|selected| item.value() == selected);
    let item_highlighted = highlighted == Some(index);
    let item_selector = item_debug_selector(selector_root, item.value());
    let item_id =
        ElementId::NamedChild(Arc::new(content_id.clone()), format!("item-{index}").into());

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
                    *theme,
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
    theme: &ArtisanTheme,
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
            *theme,
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

/// Borrowed delivery inputs for one applied select action.
struct ActionDelivery<'a, V: SelectValue> {
    action: SelectAction,
    state: &'a Rc<RefCell<SelectState>>,
    entries: &'a [SelectEntry<V>],
    on_change: Option<&'a SelectChangeHandler<V>>,
    on_open_change: Option<&'a SelectOpenChangeHandler>,
    focus: &'a FocusHandle,
    scroll_handle: &'a ScrollHandle,
    enabled: &'a [bool],
    was_open: bool,
    event: Option<&'a ClickEvent>,
    window: &'a mut Window,
    cx: &'a mut App,
}

fn notify_open_change(
    handler: Option<&SelectOpenChangeHandler>,
    open: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some(handler) = handler {
        handler(open, window, cx);
    }
}

fn apply_action<V: SelectValue>(delivery: ActionDelivery<'_, V>) {
    let ActionDelivery {
        action,
        state,
        entries,
        on_change,
        on_open_change,
        focus,
        scroll_handle,
        enabled,
        was_open,
        event,
        window,
        cx,
    } = delivery;
    match action {
        SelectAction::None => {}
        SelectAction::Open => {
            if let Some(index) = state.borrow().highlighted_index() {
                scroll_handle.scroll_to_item(index);
            }

            notify_open_change(on_open_change, true, window, cx);
        }
        SelectAction::Close => {
            window.focus(focus, cx);
            notify_open_change(on_open_change, false, window, cx);
        }
        SelectAction::Highlight(index) => {
            if enabled.get(index).copied().unwrap_or(false) {
                scroll_handle.scroll_to_item(index);
                window.refresh();
            }
        }
        SelectAction::Commit(index) => {
            if was_open {
                window.focus(focus, cx);
                notify_open_change(on_open_change, false, window, cx);
            }

            if let Some(SelectEntry::Item(item)) = entries.get(index)
                && enabled.get(index).copied().unwrap_or(false)
                && let Some(handler) = on_change
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

/// Owned render inputs shared by the select frame builders.
///
/// Cloning here is limited to `Rc` handles, small `Copy` values, and the
/// already-resolved style record; no caller data is duplicated by value.
struct SelectFrame<V: SelectValue> {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    selected: Option<V>,
    placeholder: SharedString,
    entries: Rc<Vec<SelectEntry<V>>>,
    enabled: Rc<Vec<bool>>,
    disabled: bool,
    scroll_state: SelectScrollState,
    scroll_handle: ScrollHandle,
    on_change: Option<SelectChangeHandler<V>>,
    on_open_change: Option<SelectOpenChangeHandler>,
    debug_selector: Option<SharedString>,
    state: Rc<RefCell<SelectState>>,
    style: SelectStyle,
    selector_root: String,
    selected_idx: Option<usize>,
    highlighted: Option<usize>,
    content_id: ElementId,
}

impl<V: SelectValue> SelectFrame<V> {
    fn trigger_base(&self, label: SharedString, selector: String) -> Stateful<Div> {
        let style = &self.style;
        div()
            .id(ElementId::NamedChild(
                Arc::new(self.id.clone()),
                "trigger".into(),
            ))
            .track_focus(&self.focus)
            .tab_index(0)
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .h(style.trigger_height)
            .px(style.trigger_horizontal_padding)
            .gap(style.trigger_gap)
            .rounded(style.trigger_corner_radius)
            .border(px(1.0))
            .border_color(style.trigger_border)
            .bg(style.trigger_background)
            .text_color(if self.selected_idx.is_none() {
                style.placeholder_foreground
            } else {
                style.trigger_foreground
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .whitespace_nowrap()
                    .child(label),
            )
            .child(icon(IconStyle::resolve(
                self.theme,
                AssetId::TABLER_CHEVRON_DOWN,
                IconSize::Compact,
                IconTint::Muted,
            )))
            .debug_selector(move || selector.clone())
    }

    fn trigger(&self) -> Stateful<Div> {
        let selected_idx = self.selected_idx;
        let entries = &self.entries;
        let placeholder = &self.placeholder;
        let selector_root = &self.selector_root;
        let state = &self.state;
        let on_change = &self.on_change;
        let on_open_change = &self.on_open_change;
        let focus = &self.focus;
        let scroll_handle = &self.scroll_handle;
        let disabled = self.disabled;
        // Trigger row: committed label or placeholder plus selector glyph.
        let trigger_label = selected_idx
            .and_then(|index| entries.get(index))
            .and_then(|entry| entry.as_item())
            .map_or_else(|| placeholder.clone(), |item| item.label.clone());
        let trigger_selector = format!("{selector_root}-trigger");
        let trigger_state = Rc::clone(state);
        let trigger_entries = Rc::clone(entries);
        let trigger_enabled = Rc::clone(&self.enabled);
        let trigger_open = on_open_change.clone();
        let trigger_change = on_change.clone();
        let trigger_focus = focus.clone();
        let trigger_scroll = scroll_handle.clone();
        let trigger_escape_state = Rc::clone(state);
        let trigger_escape_enabled = Rc::clone(&self.enabled);
        let trigger_escape_open = on_open_change.clone();
        let trigger_escape_focus = focus.clone();
        let state_for_keys = Rc::clone(state);
        let entries_for_keys = Rc::clone(entries);
        let enabled_for_keys = Rc::clone(&self.enabled);
        let on_change_for_keys = on_change.clone();
        let on_open_change_for_keys = on_open_change.clone();
        let focus_for_keys = focus.clone();
        let scroll_for_keys = scroll_handle.clone();
        let mut trigger = self.trigger_base(trigger_label, trigger_selector);
        if disabled {
            trigger = trigger.opacity(DISABLED_OPACITY);
        } else {
            trigger = trigger
                .on_click(
                    move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let was_open = trigger_state.borrow().is_open();
                        let action = trigger_state.borrow_mut().toggle(&trigger_enabled);
                        apply_action(ActionDelivery {
                            action,
                            state: &trigger_state,
                            entries: &trigger_entries,
                            on_change: trigger_change.as_ref(),
                            on_open_change: trigger_open.as_ref(),
                            focus: &trigger_focus,
                            scroll_handle: &trigger_scroll,
                            enabled: &trigger_enabled,
                            was_open,
                            event: Some(event),
                            window,
                            cx,
                        });
                        window.refresh();
                    },
                )
                .on_key_down(
                    move |event: &KeyDownEvent, window: &mut Window, cx: &mut App| {
                        if event.keystroke.modifiers.modified() {
                            return;
                        }
                        // Escape always announces the closed state, even when
                        // the menu is already closed: the owner may hold a
                        // stale open value that only this announcement heals.
                        if event.keystroke.key.as_str() == "escape" {
                            let _ = trigger_escape_state
                                .borrow_mut()
                                .close(&trigger_escape_enabled);
                            window.focus(&trigger_escape_focus, cx);
                            if let Some(handler) = trigger_escape_open.as_ref() {
                                handler(false, window, cx);
                            }
                            window.prevent_default();
                            cx.stop_propagation();
                            window.refresh();
                            return;
                        }
                        handle_select_key(
                            &SelectKeyContext {
                                state: Rc::clone(&state_for_keys),
                                entries: Rc::clone(&entries_for_keys),
                                enabled: Rc::clone(&enabled_for_keys),
                                on_change: on_change_for_keys.clone(),
                                on_open_change: on_open_change_for_keys.clone(),
                                focus: focus_for_keys.clone(),
                                scroll_handle: scroll_for_keys.clone(),
                            },
                            event.keystroke.key.as_str(),
                            window,
                            cx,
                        );
                    },
                );
        }
        trigger
    }

    fn viewport(&self) -> Stateful<Div> {
        let entries = &self.entries;
        let content_id = &self.content_id;
        let selector_root = &self.selector_root;
        let selected = self.selected.as_ref();
        let highlighted = self.highlighted;
        let style = &self.style;
        let theme = &self.theme;
        let disabled = self.disabled;
        let state = &self.state;
        let on_change = &self.on_change;
        let on_open_change = &self.on_open_change;
        let focus = &self.focus;
        let scroll_handle = &self.scroll_handle;
        let viewport_id = ElementId::NamedChild(Arc::new(content_id.clone()), "viewport".into());
        let viewport_selector = format!("{selector_root}-viewport");
        let mut viewport = div()
            .id(viewport_id)
            .w_full()
            .max_h(style.content_max_height)
            .overflow_y_scroll()
            .track_scroll(scroll_handle)
            .debug_selector(move || viewport_selector.clone());
        for (index, entry) in entries.iter().enumerate() {
            match entry {
                SelectEntry::Group(heading) => {
                    viewport = viewport.child(
                        div()
                            .px(style.item_horizontal_padding)
                            .py(style.item_vertical_padding)
                            .text_color(style.group_label_foreground)
                            .child(heading.clone()),
                    );
                }
                SelectEntry::Separator => {
                    viewport = viewport.child(div().h(px(1.0)).bg(style.separator_color));
                }
                SelectEntry::Item(item) => {
                    let row = render_item(&RenderItemArgs {
                        theme,
                        item,
                        index,
                        content_id,
                        selector_root,
                        selected,
                        highlighted,
                        style,
                    });
                    if item.is_disabled() || disabled {
                        viewport = viewport.child(row);
                    } else {
                        let row_state = Rc::clone(state);
                        let row_entries = Rc::clone(entries);
                        let row_enabled = Rc::clone(&self.enabled);
                        let row_change = on_change.clone();
                        let row_open = on_open_change.clone();
                        let row_focus = focus.clone();
                        let row_scroll = scroll_handle.clone();
                        viewport = viewport.child(row.on_click(
                            move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                                let was_open = row_state.borrow().is_open();
                                let action = row_state.borrow_mut().commit(index, &row_enabled);
                                apply_action(ActionDelivery {
                                    action,
                                    state: &row_state,
                                    entries: &row_entries,
                                    on_change: row_change.as_ref(),
                                    on_open_change: row_open.as_ref(),
                                    focus: &row_focus,
                                    scroll_handle: &row_scroll,
                                    enabled: &row_enabled,
                                    was_open,
                                    event: Some(event),
                                    window,
                                    cx,
                                });
                                window.refresh();
                            },
                        ));
                    }
                }
            }
        }
        viewport
    }

    fn content(&self, viewport: Stateful<Div>) -> Stateful<Div> {
        let style = &self.style;
        let theme = &self.theme;
        let content_id = &self.content_id;
        let selector_root = &self.selector_root;
        let scroll_handle = &self.scroll_handle;
        let content_selector = format!("{selector_root}-content");
        let mut content = div()
            .id(content_id.clone())
            .w_full()
            .rounded(style.content_corner_radius)
            .bg(style.content_background)
            .text_color(style.content_foreground)
            .shadow(style.content_shadow.to_vec())
            .overflow_hidden()
            .debug_selector(move || content_selector.clone());
        if self.scroll_state.can_scroll_up() {
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
        if self.scroll_state.can_scroll_down() {
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
        content
    }

    fn root(&self, trigger: Stateful<Div>, content: Stateful<Div>) -> Div {
        let state = &self.state;
        let focus = &self.focus;
        let scroll_handle = &self.scroll_handle;
        let enabled = &self.enabled;
        let on_open_change = &self.on_open_change;
        let debug_selector = &self.debug_selector;
        let mut root = div().relative().w_full().child(trigger);
        if state.borrow().is_open() {
            let outside_state = Rc::clone(state);
            let outside_focus = focus.clone();
            let outside_scroll = scroll_handle.clone();
            let outside_enabled = Rc::clone(enabled);
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
                    window.focus(&outside_focus, cx);
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
        root
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
            scroll_state,
            scroll_handle,
            on_change,
            on_open_change,
            debug_selector,
            interaction_state,
        } = self;
        let style = SelectStyle::resolve(theme, size);
        let enabled = entry_enabled_flags(&entries);
        let selected_idx = selected_index(&entries, selected.as_ref());
        let state = interaction_state.unwrap_or_else(|| {
            Rc::new(RefCell::new(SelectState::new(open, selected_idx, &enabled)))
        });
        // Reconcile only when the controlled props actually changed since the
        // last reconciliation. View-initiated transitions (toggle, activation)
        // mutate the shared state optimistically; reconciling those away on
        // the next render would revert them before the owner applies them.
        {
            let mut shared = state.borrow_mut();
            if shared.reconciled != Some((open, selected_idx)) {
                shared.reconcile(open, selected_idx, &enabled);
                shared.reconciled = Some((open, selected_idx));
            }
        }

        let entries = Rc::new(entries);
        let enabled = Rc::new(enabled);
        let selector_root = debug_selector
            .as_ref()
            .map_or_else(|| "artisan-select".to_owned(), ToString::to_string);
        let highlighted = state.borrow().highlighted_index();
        let content_id = ElementId::NamedChild(Arc::new(id.clone()), "content".into());
        let frame = SelectFrame {
            id,
            focus,
            theme,
            selected,
            placeholder,
            entries,
            enabled,
            disabled,
            scroll_state,
            scroll_handle,
            on_change,
            on_open_change,
            debug_selector,
            state,
            style,
            selector_root,
            selected_idx,
            highlighted,
            content_id,
        };
        let trigger = frame.trigger();
        let viewport = frame.viewport();
        let content = frame.content(viewport);
        frame.root(trigger, content)
    }
}

/// Shared delivery context for trigger keystrokes.
struct SelectKeyContext<V: SelectValue> {
    state: Rc<RefCell<SelectState>>,
    entries: Rc<Vec<SelectEntry<V>>>,
    enabled: Rc<Vec<bool>>,
    on_change: Option<SelectChangeHandler<V>>,
    on_open_change: Option<SelectOpenChangeHandler>,
    focus: FocusHandle,
    scroll_handle: ScrollHandle,
}

/// Handles one trigger keystroke through the shared engine and applies the
/// resulting action with keyboard (event-less) delivery.
fn handle_select_key<V: SelectValue>(
    context: &SelectKeyContext<V>,
    key_name: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let mapped = SelectKey::from_key_name(key_name);
    let key = if let Some(key) = mapped {
        key
    } else {
        let mut chars = key_name.chars();
        match (chars.next(), chars.next()) {
            (Some(character), None) if !character.is_control() => SelectKey::Character(character),
            _ => return,
        }
    };
    let was_open = context.state.borrow().is_open();
    let action = context
        .state
        .borrow_mut()
        .handle_key_at(key, &context.entries, monotonic_now());
    if action.is_effective() {
        apply_action(ActionDelivery {
            action,
            state: &context.state,
            entries: &context.entries,
            on_change: context.on_change.as_ref(),
            on_open_change: context.on_open_change.as_ref(),
            focus: &context.focus,
            scroll_handle: &context.scroll_handle,
            enabled: &context.enabled,
            was_open,
            event: None,
            window,
            cx,
        });
        window.refresh();
    }
}

/// Returns a stable hexadecimal hash suffix for one caller-owned value.
///
/// Hashing keeps arbitrary caller data out of selector text while remaining
/// deterministic within one process.
#[must_use]
pub fn stable_value_selector_suffix<V: Hash + ?Sized>(value: &V) -> String {
    let mut hasher = SelectFnv1aHasher(0xcbf2_9ce4_8422_2325);
    value.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Returns the stable debug selector for one item value under a root.
#[must_use]
pub fn item_debug_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    format!(
        "{root_selector}-item-{}",
        stable_value_selector_suffix(value)
    )
}

/// Change notification for a committed select value.
pub type SelectChangeHandler<V> = Rc<dyn Fn(V, Option<&ClickEvent>, &mut Window, &mut App)>;

/// Change notification for controlled open updates.
pub type SelectOpenChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Semantic trigger/listbox snapshot for inspectable metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectSemanticState {
    /// Fixed trigger role name.
    pub trigger_role: &'static str,
    /// Fixed listbox role name.
    pub listbox_role: &'static str,
    /// Fixed option role name.
    pub option_role: &'static str,
    /// Whether the listbox is currently expanded.
    pub expanded: bool,
    /// Whether the whole control is disabled.
    pub disabled: bool,
    /// Visible trigger text.
    pub label: SharedString,
    /// Whether the trigger shows the placeholder.
    pub placeholder: bool,
    /// Visible label of the committed value, if any.
    pub value_label: Option<SharedString>,
    /// Stable trigger selector.
    pub trigger_selector: String,
    /// Stable content selector.
    pub content_selector: String,
}

/// Semantic record for one rendered option row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectItemSemanticState {
    /// Fixed option role name.
    pub role: &'static str,
    /// Visible option label.
    pub label: SharedString,
    /// Whether this row holds the committed value.
    pub selected: bool,
    /// Whether this row holds the roving highlight.
    pub highlighted: bool,
    /// Whether this row can be activated.
    pub disabled: bool,
    /// Stable per-value selector.
    pub selector: String,
}

/// A controlled native select/listbox view.
///
/// The caller owns `open` and the committed value; interaction state flows
/// through an optional shared [`SelectState`]. Rendering reconciles the
/// shared state with the controlled props every frame, so owner-applied
/// updates always win while in-progress keyboard motion survives renders
/// that change nothing.
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
    scroll_state: SelectScrollState,
    scroll_handle: ScrollHandle,
    on_change: Option<SelectChangeHandler<V>>,
    on_open_change: Option<SelectOpenChangeHandler>,
    debug_selector: Option<SharedString>,
    interaction_state: Option<Rc<RefCell<SelectState>>>,
}

impl<V: SelectValue> Select<V> {
    /// Creates a select with an explicit committed value.
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        selected: Option<V>,
        entries: Vec<SelectEntry<V>>,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            selected,
            open: false,
            placeholder: SharedString::from(String::new()),
            entries,
            size: SelectSize::default(),
            disabled: false,
            scroll_state: SelectScrollState::default(),
            scroll_handle: ScrollHandle::new(),
            on_change: None,
            on_open_change: None,
            debug_selector: None,
            interaction_state: None,
        }
    }

    /// Sets the controlled open state.
    #[must_use]
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Sets the placeholder shown without a committed value.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Adds a stable selector; trigger, content, and viewport derive
    /// `{selector}-trigger`, `{selector}-content`, and `{selector}-viewport`.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Selects the trigger size.
    #[must_use]
    pub fn size(mut self, size: SelectSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the whole-control disabled state.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the caller-owned scroll-edge state.
    #[must_use]
    pub fn scroll_state(mut self, scroll_state: SelectScrollState) -> Self {
        self.scroll_state = scroll_state;
        self
    }

    /// Shares deterministic interaction state across renders.
    #[must_use]
    pub fn with_interaction_state(mut self, state: Rc<RefCell<SelectState>>) -> Self {
        self.interaction_state = Some(state);
        self
    }

    /// Installs the committed-value notification.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(V, Option<&ClickEvent>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Installs the controlled open-update notification.
    #[must_use]
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }

    /// Returns the committed entries.
    #[must_use]
    pub fn entries(&self) -> &[SelectEntry<V>] {
        &self.entries
    }

    /// Returns the committed value's visible label, if a row holds it.
    #[must_use]
    pub fn current_label(&self) -> Option<&str> {
        selected_index(&self.entries, self.selected.as_ref())
            .and_then(|index| self.entries.get(index))
            .and_then(|entry| entry.as_item())
            .map(SelectItem::label)
    }

    /// Returns the trigger text: committed label or placeholder.
    #[must_use]
    pub fn display_label(&self) -> String {
        self.current_label()
            .unwrap_or_else(|| self.placeholder.as_ref())
            .to_owned()
    }

    /// Resolves the trigger recipe for the requested size.
    #[must_use]
    pub fn visual_style(&self) -> SelectStyle {
        SelectStyle::resolve(self.theme, self.size)
    }

    /// Returns the caller-owned scroll-edge state.
    #[must_use]
    pub const fn scroll_state_value(&self) -> SelectScrollState {
        self.scroll_state
    }

    /// Returns the semantic trigger/listbox snapshot.
    #[must_use]
    pub fn semantic_state(&self) -> SelectSemanticState {
        let value_label = self
            .current_label()
            .map(|label| SharedString::from(label.to_owned()));
        let root = self.selector_root();
        SelectSemanticState {
            trigger_role: "combobox",
            listbox_role: "listbox",
            option_role: "option",
            expanded: self.open && !self.disabled,
            disabled: self.disabled,
            label: SharedString::from(self.display_label()),
            placeholder: value_label.is_none(),
            value_label,
            trigger_selector: format!("{root}-trigger"),
            content_selector: format!("{root}-content"),
        }
    }

    /// Returns semantic item records in visual item order.
    #[must_use]
    pub fn item_semantics(&self, state: &SelectState) -> Vec<SelectItemSemanticState> {
        let root = self.selector_root();
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                SelectEntry::Item(item) => Some(SelectItemSemanticState {
                    role: "option",
                    label: item.label.clone(),
                    selected: self
                        .selected
                        .as_ref()
                        .is_some_and(|selected| item.value() == selected),
                    highlighted: state.highlighted_index().is_some_and(|highlighted| {
                        self.entries.get(highlighted).is_some_and(|row| {
                            matches!(row, SelectEntry::Item(row) if row.value() == item.value())
                        })
                    }),
                    disabled: self.disabled || item.is_disabled(),
                    selector: item_debug_selector(&root, item.value()),
                }),
                SelectEntry::Group(_) | SelectEntry::Separator => None,
            })
            .collect()
    }

    fn selector_root(&self) -> String {
        self.debug_selector
            .as_ref()
            .map_or_else(|| "artisan-select".to_owned(), ToString::to_string)
    }
}
