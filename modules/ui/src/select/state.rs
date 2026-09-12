//! Model types and the deterministic controlled state machine for `Select`.
//!
//! Split out of `select.rs`; the GPUI render frame and the `Select` builder
//! live in the parent module.

#![forbid(unsafe_code)]

use std::hash::Hash;
use std::time::Duration;

use gpui::SharedString;

use super::TYPEAHEAD_TIMEOUT;
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
    pub(super) label: SharedString,
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

    /// Reconciles a fresh controlled render exactly once per controlled prop
    /// change.
    ///
    /// View-initiated transitions mutate the shared state optimistically;
    /// reconciling those away on the next render would revert them before the
    /// owner applies them.
    pub(super) fn reconcile_controlled(
        &mut self,
        open: bool,
        selected_index: Option<usize>,
        enabled: &[bool],
    ) {
        if self.reconciled != Some((open, selected_index)) {
            self.reconcile(open, selected_index, enabled);
            self.reconciled = Some((open, selected_index));
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

pub(super) fn entry_enabled_flags<V: SelectValue>(entries: &[SelectEntry<V>]) -> Vec<bool> {
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
