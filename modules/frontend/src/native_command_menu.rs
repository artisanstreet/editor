//! Native GPUI command menu for the rail (`Cmd/Ctrl+K` palette).
//!
//! Native counterpart of `routes/components/command-menu.svelte`, minus the
//! Effect controllers, router, and draft preparation, which the orchestrator
//! owns. The legacy surface is a Bits `CommandDialog` with a `CommandInput`
//! (`placeholder="Search threads and actions…"`, `CommandEmpty` copy `"No
//! results found."`), a static `Actions` group (`New thread`, `Open
//! settings`), and one `CommandGroup` per project holding that project's
//! threads with the searchable value
//! `` `${display_title} ${thread.title} ${thread.thread_id}` ``.
//!
//! [`CommandMenuState`] is the dependency-free interaction contract: open
//! trigger state, controlled query, ranked groups, keyboard movement, and
//! single-activation emission. Ranking reuses
//! [`crate::command_ranking::filter_and_rank_groups`] so matching, scoring,
//! and group ordering cannot drift from the audited scorer. [`NativeCommandMenu`]
//! is the thin GPUI view over that state.
//!
//! Fidelity mapping (legacy element → this module, Tailwind → Styled notes):
//!
//! - `CommandDialog` → the active native route uses one anchored titlebar
//!   dropdown; the retained centered `deferred` card remains available to
//!   callers of the legacy presentation and uses the shared
//!   [`artisan_ui::popover`] recipe.
//! - `CommandInput` → [`artisan_ui::input::Input`] plus
//!   [`NativeCommandMenuInputElement`], which registers GPUI's native text
//!   service for typing, paste, selection, and IME composition.
//! - `CommandGroup[heading]` → heading text rows in group order; the ranked
//!   group order comes from the scorer, matching Bits' `filter` + rank
//!   behavior. `rounded-sm` rows map onto the shared
//!   [`artisan_ui::list_row`] menu recipe (`px-2 py-1.5`, `--radius-xl`);
//!   the ramp step differs from `rounded-sm` and is named in code.
//! - `CommandItem` leading glyphs → [`artisan_ui::icon::icon`]: `Edit`
//!   becomes `TABLER_EDIT`, `Settings` becomes `TABLER_SETTINGS`, and
//!   `MessageCircle` becomes `TABLER_MESSAGE_CIRCLE`.
//! - `CommandEmpty` → the exact `"No results found."` copy in muted text.
//! - Toggle shortcut (`meta/ctrl+k`) → [`CommandMenuState::press_toggle`]
//!   plus [`CommandMenuState::matches_toggle_shortcut`]; the application binds
//!   the same titlebar entity with an open-or-focus command and restores the
//!   application root after dismissal.
//! - Activation (`StartNewThread` navigation, settings link, thread links) →
//!   [`CommandMenuAction`], drained once through
//!   [`CommandMenuState::take_pending_action`]; navigation and draft effects
//!   stay with the orchestrator.
//!
//! Deliberately absent: [`artisan_ui::button`] and [`artisan_ui::badge`]
//! have no counterpart in the legacy command menu (its trigger lives outside
//! this surface and its rows carry no badges), so they are not used here.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{ops::Range, panic};

use artisan_assets::AssetId;
use artisan_ui::{
    asset_seam::asset_glyph,
    icon::{IconSize, IconStyle, IconTint, icon},
    input::Input,
    list_row::{ListRowContent, ListRowGeometry, ListRowStyle, ListRowTone, list_row},
    popover::{PopoverStyle, popover_content},
    theme::{ArtisanTheme, DesktopTheme, ThemeMode},
};
use gpui::{
    AnyElement, App, Bounds, ClickEvent, ClipboardItem, Context, Div, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, FontWeight, GlobalElementId,
    InspectorElementId, InteractiveElement as _, KeyBinding, KeyDownEvent, LayoutId,
    ParentElement as _, Pixels, Point, Render, ScrollHandle, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled as _, UTF16Selection, Window, actions, deferred, div,
    prelude::{FluentBuilder as _, IntoElement},
    px,
};

use crate::command_ranking::{CommandGroup, CommandItem, filter_and_rank_groups};

actions!(
    native_command_menu,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Paste,
        Copy,
        Cut,
        Home,
        End,
    ]
);

/// Stable debug selector for the command-menu root.
pub const COMMAND_MENU_SELECTOR: &str = "artisan-native-command-menu";
/// Stable debug selector for the query input branch.
pub const COMMAND_MENU_INPUT_SELECTOR: &str = "artisan-native-command-menu-input";
/// Stable debug selector for the ranked list branch.
pub const COMMAND_MENU_LIST_SELECTOR: &str = "artisan-native-command-menu-list";
/// Stable debug selector for the anchored titlebar dropdown.
pub const COMMAND_MENU_DROPDOWN_SELECTOR: &str = "artisan-native-command-menu-dropdown";
/// Prefix for the stable selectors painted on command rows.
pub const COMMAND_MENU_ROW_SELECTOR_PREFIX: &str = "artisan-native-command-menu-row";
/// Exact legacy input placeholder (`command-menu.svelte`).
pub const COMMAND_MENU_PLACEHOLDER: &str = "Search threads and actions…";
/// Exact legacy empty-list copy (`CommandEmpty`).
pub const COMMAND_MENU_EMPTY_LABEL: &str = "No results found.";
/// The toggle shortcut key (`meta/ctrl+k` in the legacy window handler).
pub const COMMAND_MENU_SHORTCUT_KEY: &str = "k";
/// Key context for the shared titlebar query input.
const COMMAND_MENU_KEY_CONTEXT: &str = "artisan-native-command-menu";
/// Stable identity of the static actions group.
pub const ACTIONS_GROUP_ID: &str = "actions";
/// Exact legacy heading of the static actions group.
pub const ACTIONS_GROUP_HEADING: &str = "Actions";
/// Stable identity of the new-thread action row.
pub const NEW_THREAD_ITEM_ID: &str = "new-thread";
/// Stable identity of the open-settings action row.
pub const OPEN_SETTINGS_ITEM_ID: &str = "open-settings";
/// Prefix used for project rows in the live project catalog.
pub const PROJECT_ITEM_ID_PREFIX: &str = "project-";
/// Exact legacy new-thread action label.
pub const NEW_THREAD_LABEL: &str = "New task";
/// Exact legacy open-settings action label.
pub const OPEN_SETTINGS_LABEL: &str = "Open settings";
/// Heading used for threads without a project (`command-menu.svelte`).
pub const UNASSIGNED_GROUP_HEADING: &str = "Unassigned";

/// Preferred dialog width in logical pixels (legacy `sm:max-w-lg`, 32 rem).
const MENU_WIDTH_PX: f32 = 512.0;
/// Bounded list height so long thread catalogs scroll instead of overflowing.
const MENU_LIST_MAX_HEIGHT_PX: f32 = 320.0;
/// Dialog top offset keeps the palette in the upper half like a command bar.
const MENU_TOP_OFFSET_PX: f32 = 96.0;

/// One action emitted after a command row has been activated.
///
/// The orchestrator dispatches the action (navigation, draft preparation);
/// this surface never routes or mutates host state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandMenuAction {
    /// Jump to the root new-thread draft (legacy `StartNewThread`).
    NewThread,
    /// Open the settings surface (legacy `/settings/models` link).
    OpenSettings,
    /// Open one listed project and keep its real task catalog in view.
    OpenProject {
        /// Durable project identity of the activated row.
        project_id: String,
    },
    /// Open one listed thread.
    OpenThread {
        /// Durable thread identity of the activated row.
        thread_id: String,
    },
}

/// One searchable row owned by a [`CommandMenuGroup`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandMenuEntry {
    /// Stable row identity within its group.
    pub id: String,
    /// Reader-facing row label.
    pub title: String,
    /// Additional match-only keywords (stored title, thread identity).
    pub keywords: Vec<String>,
    /// Typed action emitted on activation.
    pub action: CommandMenuAction,
}

impl CommandMenuEntry {
    /// Builds the static new-thread action row.
    #[must_use]
    pub fn new_thread() -> Self {
        Self {
            id: String::from(NEW_THREAD_ITEM_ID),
            title: String::from(NEW_THREAD_LABEL),
            keywords: Vec::new(),
            action: CommandMenuAction::NewThread,
        }
    }

    /// Builds the static open-settings action row.
    #[must_use]
    pub fn open_settings() -> Self {
        Self {
            id: String::from(OPEN_SETTINGS_ITEM_ID),
            title: String::from(OPEN_SETTINGS_LABEL),
            keywords: Vec::new(),
            action: CommandMenuAction::OpenSettings,
        }
    }

    /// Builds one project row from the live project catalog.
    #[must_use]
    pub fn project(project_id: impl Into<String>, title: impl Into<String>) -> Self {
        let project_id = project_id.into();
        Self {
            id: format!("{PROJECT_ITEM_ID_PREFIX}{project_id}"),
            title: title.into(),
            keywords: vec![project_id.clone()],
            action: CommandMenuAction::OpenProject { project_id },
        }
    }

    /// Builds one thread row whose searchable text mirrors the legacy
    /// `value`: the display title plus the stored title and thread identity
    /// as match-only keywords.
    #[must_use]
    pub fn thread(
        thread_id: impl Into<String>,
        display_title: impl Into<String>,
        stored_title: impl Into<String>,
    ) -> Self {
        let thread_id = thread_id.into();
        Self {
            id: thread_id.clone(),
            title: display_title.into(),
            keywords: vec![stored_title.into(), thread_id.clone()],
            action: CommandMenuAction::OpenThread { thread_id },
        }
    }
}

/// One ordered group of searchable rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandMenuGroup {
    /// Stable group identity.
    pub id: String,
    /// Visible heading rendered above the group's rows.
    pub heading: String,
    /// Rows in caller-supplied (catalog) order.
    pub entries: Vec<CommandMenuEntry>,
}

impl CommandMenuGroup {
    /// Builds a group from its identity, heading, and rows.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        heading: impl Into<String>,
        entries: Vec<CommandMenuEntry>,
    ) -> Self {
        Self {
            id: id.into(),
            heading: heading.into(),
            entries,
        }
    }

    /// Builds the static actions group (`New thread`, `Open settings`).
    #[must_use]
    pub fn actions() -> Self {
        Self::new(
            ACTIONS_GROUP_ID,
            ACTIONS_GROUP_HEADING,
            vec![
                CommandMenuEntry::new_thread(),
                CommandMenuEntry::open_settings(),
            ],
        )
    }
}

/// One ranked row in display order.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibleCommandRow {
    /// Position of the owning group in [`CommandMenuState::groups`].
    pub group: usize,
    /// Position of the entry in its group's `entries`.
    pub index: usize,
    /// Finite rank from [`crate::command_ranking`] (`0.0..=1.0`).
    pub score: f64,
}

/// One non-empty ranked group in display order.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibleCommandGroup {
    /// Position of the group in [`CommandMenuState::groups`].
    pub group: usize,
    /// Kept rows in descending rank.
    pub rows: Vec<VisibleCommandRow>,
}

/// One activation waiting for orchestrator observation.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingCommand {
    group_id: String,
    item_id: String,
    action: CommandMenuAction,
}

/// Explicit interaction state for the command menu.
///
/// Pure and deterministic: filtering delegates to
/// [`crate::command_ranking`], and effects leave only through
/// [`Self::take_pending_action`], so the whole contract is assertable
/// without a window.
#[derive(Debug)]
pub struct CommandMenuState {
    groups: Vec<CommandMenuGroup>,
    open: bool,
    disabled: bool,
    query: String,
    highlight: Option<usize>,
    pending: Option<PendingCommand>,
}

impl CommandMenuState {
    /// Builds a closed menu over the supplied groups.
    #[must_use]
    pub fn new(groups: Vec<CommandMenuGroup>) -> Self {
        let mut state = Self {
            groups,
            open: false,
            disabled: false,
            query: String::new(),
            highlight: None,
            pending: None,
        };
        state.highlight = state.first_flat();
        state
    }

    /// Returns whether `key` with the supplied modifiers matches the legacy
    /// window toggle (`meta/ctrl+k`).
    #[must_use]
    pub fn matches_toggle_shortcut(key: &str, platform: bool, control: bool) -> bool {
        key == COMMAND_MENU_SHORTCUT_KEY && (platform || control)
    }

    /// Returns the groups in caller-supplied order.
    #[must_use]
    pub fn groups(&self) -> &[CommandMenuGroup] {
        &self.groups
    }

    /// Returns the controlled query text.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns whether the dialog is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Returns whether the menu refuses interaction.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Sets the disabled state. Disabling closes the menu and discards the
    /// one activation that has not yet been consumed.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
        if disabled {
            self.close_without_action();
            self.pending = None;
        }
    }

    /// Replaces the groups wholesale (catalog push path). The highlight
    /// resets to the first visible row; a pending activation survives only
    /// while its row still exists.
    pub fn replace_groups(&mut self, groups: Vec<CommandMenuGroup>) {
        self.groups = groups;
        if let Some(pending) = self.pending.take() {
            let still_listed = self.groups.iter().any(|group| group.id == pending.group_id)
                && self
                    .visible_groups()
                    .iter()
                    .flat_map(|group| &group.rows)
                    .any(|row| {
                        self.entry_at(row)
                            .is_some_and(|entry| entry.id == pending.item_id)
                    });
            if still_listed {
                self.pending = Some(pending);
            }
        }
        self.highlight = self.first_flat();
    }

    /// Opens the dialog with a fresh query unless disabled or an activation
    /// is still waiting to be consumed.
    pub fn open(&mut self) {
        if self.disabled || self.pending.is_some() {
            return;
        }
        self.query.clear();
        self.highlight = self.first_flat();
        self.open = true;
    }

    /// Dismisses the dialog without emitting an action.
    pub fn dismiss(&mut self) {
        self.close_without_action();
    }

    /// Toggles the dialog: the open-trigger contract behind `Cmd/Ctrl+K`.
    pub fn press_toggle(&mut self) {
        if self.disabled || self.pending.is_some() {
            return;
        }
        if self.open {
            self.close_without_action();
        } else {
            self.open();
        }
    }

    /// Replaces the controlled query and resets the highlight to the first
    /// visible row, matching the Bits filter reset.
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.highlight = self.first_flat();
    }

    /// Appends one typed character to the query (key handler path).
    pub fn push_query_char(&mut self, character: char) {
        if !self.open {
            return;
        }
        self.query.push(character);
        self.highlight = self.first_flat();
    }

    /// Removes the last query character, returning whether one existed.
    pub fn pop_query_char(&mut self) -> bool {
        if !self.open {
            return false;
        }
        let popped = self.query.pop().is_some();
        if popped {
            self.highlight = self.first_flat();
        }
        popped
    }

    /// Returns the ranked, non-empty groups in display order.
    #[must_use]
    pub fn visible_groups(&self) -> Vec<VisibleCommandGroup> {
        let scorer_groups: Vec<CommandGroup<usize, (usize, usize)>> = self
            .groups
            .iter()
            .enumerate()
            .map(|(group_index, group)| {
                CommandGroup::new(
                    group_index,
                    group
                        .entries
                        .iter()
                        .enumerate()
                        .map(|(entry_index, entry)| {
                            CommandItem::with_keywords(
                                (group_index, entry_index),
                                entry.title.clone(),
                                entry.keywords.clone(),
                            )
                        })
                        .collect(),
                )
            })
            .collect();
        filter_and_rank_groups(scorer_groups, &self.query)
            .into_iter()
            .map(|ranked| VisibleCommandGroup {
                group: ranked.id,
                rows: ranked
                    .items
                    .into_iter()
                    .map(|item| VisibleCommandRow {
                        group: item.item.0,
                        index: item.item.1,
                        score: item.score,
                    })
                    .collect(),
            })
            .collect()
    }

    /// Returns the number of visible rows across all ranked groups.
    #[must_use]
    pub fn row_count(&self) -> usize {
        self.visible_groups()
            .iter()
            .map(|group| group.rows.len())
            .sum()
    }

    /// Returns the highlighted row's flat address, if any row is visible.
    #[must_use]
    pub fn highlighted_flat(&self) -> Option<usize> {
        self.highlight
    }

    /// Returns the highlighted entry's typed action, if one is highlighted.
    #[must_use]
    pub fn highlighted_action(&self) -> Option<CommandMenuAction> {
        let flat = self.highlight?;
        self.entry_at_flat(flat).map(|entry| entry.action.clone())
    }

    /// Moves the highlight down one row, wrapping past the final row.
    pub fn move_next(&mut self) {
        self.advance(true);
    }

    /// Moves the highlight up one row, wrapping past the first row.
    pub fn move_previous(&mut self) {
        self.advance(false);
    }

    /// Jumps to the first visible row (Home).
    pub fn move_first(&mut self) {
        if self.open {
            self.highlight = self.first_flat();
        }
    }

    /// Jumps to the final visible row (End).
    pub fn move_last(&mut self) {
        if self.open {
            self.highlight = self.last_flat();
        }
    }

    /// Retargets the highlight to one stable row identity (pointer path).
    /// Returns whether the row is currently visible.
    pub fn apply_highlight_id(&mut self, group_id: &str, item_id: &str) -> bool {
        let Some(flat) = self.flat_for_id(group_id, item_id) else {
            return false;
        };
        self.highlight = Some(flat);
        true
    }

    /// Activates the highlighted row with Enter semantics.
    ///
    /// Closing happens strictly before the action is queued, and at most one
    /// activation waits unconsumed (single-activation): further activations
    /// return `None` until [`Self::take_pending_action`] drains it.
    pub fn activate_highlighted(&mut self) -> Option<CommandMenuAction> {
        let flat = self.highlight?;
        self.activate_flat(flat)
    }

    /// Activates one flat visible row (pointer selection).
    pub fn activate_row(&mut self, flat: usize) -> Option<CommandMenuAction> {
        self.activate_flat(flat)
    }

    /// Activates one stable row identity (view callback path).
    pub fn activate_id(&mut self, group_id: &str, item_id: &str) -> Option<CommandMenuAction> {
        let flat = self.flat_for_id(group_id, item_id)?;
        self.activate_flat(flat)
    }

    /// Returns and clears the one activation waiting for orchestrator
    /// observation.
    pub fn take_pending_action(&mut self) -> Option<CommandMenuAction> {
        self.pending.take().map(|pending| pending.action)
    }

    /// Returns the pending activation without consuming it.
    #[must_use]
    pub fn pending_action(&self) -> Option<&CommandMenuAction> {
        self.pending.as_ref().map(|pending| &pending.action)
    }

    fn close_without_action(&mut self) {
        self.open = false;
        self.highlight = None;
    }

    fn entry_at(&self, row: &VisibleCommandRow) -> Option<&CommandMenuEntry> {
        self.groups.get(row.group)?.entries.get(row.index)
    }

    fn entry_at_flat(&self, flat: usize) -> Option<&CommandMenuEntry> {
        let mut remaining = flat;
        for group in self.visible_groups() {
            if remaining < group.rows.len() {
                return self.entry_at(&group.rows[remaining]);
            }
            remaining -= group.rows.len();
        }
        None
    }

    fn flat_for_id(&self, group_id: &str, item_id: &str) -> Option<usize> {
        let mut flat = 0;
        for group in self.visible_groups() {
            for row in &group.rows {
                let matches = self.entry_at(row).is_some_and(|entry| entry.id == item_id)
                    && self.groups.get(row.group).is_some_and(|g| g.id == group_id);
                if matches {
                    return Some(flat);
                }
                flat += 1;
            }
        }
        None
    }

    fn first_flat(&self) -> Option<usize> {
        if self.row_count() == 0 { None } else { Some(0) }
    }

    fn last_flat(&self) -> Option<usize> {
        let count = self.row_count();
        if count == 0 { None } else { Some(count - 1) }
    }

    fn advance(&mut self, forward: bool) {
        if !self.open {
            return;
        }
        let count = self.row_count();
        if count == 0 {
            self.highlight = None;
            return;
        }
        let current = self.highlight.unwrap_or(0).min(count - 1);
        self.highlight = Some(if forward {
            if current + 1 == count { 0 } else { current + 1 }
        } else if current == 0 {
            count - 1
        } else {
            current - 1
        });
    }

    fn activate_flat(&mut self, flat: usize) -> Option<CommandMenuAction> {
        if !self.open || self.disabled || self.pending.is_some() {
            return None;
        }
        let row: VisibleCommandRow = self
            .visible_groups()
            .into_iter()
            .flat_map(|group| group.rows)
            .nth(flat)?
            .clone();
        let entry = self.entry_at(&row)?.clone();
        let group_id = self.groups.get(row.group)?.id.clone();
        self.close_without_action();
        let action = entry.action.clone();
        self.pending = Some(PendingCommand {
            group_id,
            item_id: entry.id,
            action: action.clone(),
        });
        Some(action)
    }
}

/// A real GPUI command-menu dialog over [`CommandMenuState`].
pub struct NativeCommandMenu {
    state: CommandMenuState,
    theme: ArtisanTheme,
    desktop_theme: DesktopTheme,
    input_focus: FocusHandle,
    menu_scroll: ScrollHandle,
    anchored: bool,
    return_focus: Option<FocusHandle>,
    input_selection: Range<usize>,
    input_selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    input_bounds: Option<Bounds<Pixels>>,
}

impl NativeCommandMenu {
    /// Builds the menu over the supplied groups.
    pub fn new(groups: Vec<CommandMenuGroup>, mode: ThemeMode, cx: &mut Context<Self>) -> Self {
        Self {
            state: CommandMenuState::new(groups),
            theme: ArtisanTheme::for_mode(mode),
            desktop_theme: DesktopTheme::neutral_dark(),
            input_focus: cx.focus_handle(),
            menu_scroll: ScrollHandle::new(),
            anchored: false,
            return_focus: None,
            input_selection: 0..0,
            input_selection_reversed: false,
            marked_range: None,
            input_bounds: None,
        }
    }

    /// Switches this menu to the inline titlebar search presentation.
    #[must_use]
    pub fn titlebar_mode(mut self) -> Self {
        self.anchored = true;
        self
    }

    /// Records the application root focus target used after Escape or a row
    /// activation. The menu remains a single entity; no second modal search
    /// surface is created.
    pub fn set_return_focus(&mut self, focus: FocusHandle) {
        self.return_focus = Some(focus);
    }

    /// Read-only access to the interaction state.
    #[must_use]
    pub fn state(&self) -> &CommandMenuState {
        &self.state
    }

    /// The query input's focus handle for orchestrator focus management.
    #[must_use]
    pub fn input_focus(&self) -> &FocusHandle {
        &self.input_focus
    }

    /// Opens the dialog and moves focus to the query input.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.open();
        if self.state.is_open() {
            self.reset_input_selection();
            window.focus(&self.input_focus, cx);
        }
        cx.notify();
    }

    /// Dismisses the dialog and drops input focus back to the window.
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.dismiss();
        self.focus_return(window, cx);
        cx.notify();
    }

    /// Opens the titlebar menu when closed, or focuses its existing query when
    /// already open. Ctrl/Cmd+K therefore never creates a competing modal and
    /// never clears a query the user is actively editing.
    pub fn focus_or_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.state.is_open() {
            self.state.open();
            if self.state.is_open() {
                self.reset_input_selection();
            }
        }
        if self.state.is_open() {
            window.focus(&self.input_focus, cx);
        }
        self.reveal_highlight();
        cx.notify();
    }

    /// Toggles the dialog for the `Cmd/Ctrl+K` trigger contract.
    pub fn press_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.press_toggle();
        if self.state.is_open() {
            self.reset_input_selection();
            window.focus(&self.input_focus, cx);
        } else {
            self.focus_return(window, cx);
        }
        self.reveal_highlight();
        cx.notify();
    }

    /// Replaces the catalog groups (controller push path).
    pub fn replace_groups(&mut self, groups: Vec<CommandMenuGroup>, cx: &mut Context<Self>) {
        self.state.replace_groups(groups);
        self.reset_input_selection();
        self.reveal_highlight();
        cx.notify();
    }

    /// Sets the disabled state.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.state.set_disabled(disabled);
        cx.notify();
    }

    /// Consumes the one pending activation.
    pub fn take_pending_action(&mut self) -> Option<CommandMenuAction> {
        self.state.take_pending_action()
    }

    /// Registers the titlebar query's native text-service actions.
    pub(crate) fn bind_actions(cx: &mut App) {
        cx.bind_keys([
            KeyBinding::new("backspace", Backspace, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("delete", Delete, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("left", Left, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("right", Right, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("shift-left", SelectLeft, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("shift-right", SelectRight, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("cmd-a", SelectAll, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("ctrl-a", SelectAll, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("cmd-v", Paste, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("ctrl-v", Paste, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("cmd-c", Copy, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("ctrl-c", Copy, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("cmd-x", Cut, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("ctrl-x", Cut, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("home", Home, Some(COMMAND_MENU_KEY_CONTEXT)),
            KeyBinding::new("end", End, Some(COMMAND_MENU_KEY_CONTEXT)),
        ]);
    }

    fn reset_input_selection(&mut self) {
        let end = self.state.query().len();
        self.input_selection = end..end;
        self.input_selection_reversed = false;
        self.marked_range = None;
    }

    fn cursor_offset(&self) -> usize {
        if self.input_selection_reversed {
            self.input_selection.start
        } else {
            self.input_selection.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.input_selection = offset..offset;
        self.input_selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize) {
        let anchor = if self.input_selection_reversed {
            self.input_selection.end
        } else {
            self.input_selection.start
        };
        if offset < anchor {
            self.input_selection = offset..anchor;
            self.input_selection_reversed = true;
        } else {
            self.input_selection = anchor..offset;
            self.input_selection_reversed = false;
        }
    }

    fn move_left(&mut self, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.input_selection.is_empty() {
            self.input_selection.start
        } else {
            previous_character_boundary(self.state.query(), self.cursor_offset())
        };
        if extend {
            self.select_to(target);
            self.marked_range = None;
            cx.notify();
        } else {
            self.move_to(target, cx);
        }
    }

    fn move_right(&mut self, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.input_selection.is_empty() {
            self.input_selection.end
        } else {
            next_character_boundary(self.state.query(), self.cursor_offset())
        };
        if extend {
            self.select_to(target);
            self.marked_range = None;
            cx.notify();
        } else {
            self.move_to(target, cx);
        }
    }

    fn replacement_range(&self, range: Option<Range<usize>>) -> Option<Range<usize>> {
        let query = self.state.query();
        let range = match range {
            Some(range) => utf16_range_to_utf8(query, range),
            None => self
                .marked_range
                .clone()
                .or_else(|| Some(self.input_selection.clone())),
        }?;
        (range.start <= range.end
            && range.end <= query.len()
            && query.is_char_boundary(range.start)
            && query.is_char_boundary(range.end))
        .then_some(range)
    }

    fn replace_query_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
        marked_selection: Option<Range<usize>>,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_open() || self.state.is_disabled() {
            return;
        }
        let selected_offsets = match marked_selection {
            Some(selected_range) => {
                let Some(start) = utf16_offset_to_utf8(replacement, selected_range.start) else {
                    return;
                };
                let Some(end) = utf16_offset_to_utf8(replacement, selected_range.end) else {
                    return;
                };
                Some((start, end))
            }
            None => None,
        };
        let mut query = self.state.query().to_owned();
        query.replace_range(range.clone(), replacement);
        self.state.set_query(query);
        let replacement_end = range.start.saturating_add(replacement.len());
        if let Some((start, end)) = selected_offsets {
            self.input_selection = range.start + start..range.start + end;
            self.input_selection_reversed = false;
            self.marked_range = Some(range.start..replacement_end);
        } else {
            self.input_selection = replacement_end..replacement_end;
            self.input_selection_reversed = false;
            self.marked_range = None;
        }
        cx.notify();
    }

    fn delete_backward_action(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.input_selection.is_empty()).then_some(self.input_selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                (cursor > 0)
                    .then(|| previous_character_boundary(self.state.query(), cursor)..cursor)
            });
        if let Some(range) = range {
            self.replace_query_range(range, "", None, cx);
        }
    }

    fn delete_forward_action(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let range = self
            .marked_range
            .clone()
            .filter(|range| !range.is_empty())
            .or_else(|| (!self.input_selection.is_empty()).then_some(self.input_selection.clone()))
            .or_else(|| {
                let cursor = self.cursor_offset();
                (cursor < self.state.query().len())
                    .then(|| cursor..next_character_boundary(self.state.query(), cursor))
            });
        if let Some(range) = range {
            self.replace_query_range(range, "", None, cx);
        }
    }

    fn move_left_action(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.move_left(false, cx);
    }

    fn move_right_action(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.move_right(false, cx);
    }

    fn select_left_action(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_left(true, cx);
    }

    fn select_right_action(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_right(true, cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.input_selection = 0..self.state.query().len();
        self.input_selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    fn move_home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn move_end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        let end = self.state.query().len();
        self.move_to(end, cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let Some(range) = self.replacement_range(None) else {
            return;
        };
        self.replace_query_range(range, &text, None, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.input_selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.state.query()[self.input_selection.clone()].to_owned(),
        ));
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.input_selection.is_empty() {
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(
            self.state.query()[self.input_selection.clone()].to_owned(),
        ));
        let range = self.input_selection.clone();
        self.replace_query_range(range, "", None, cx);
    }

    fn focus_return(&self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(focus) = &self.return_focus {
            window.focus(focus, cx);
        }
    }

    fn handle_trigger_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.stop_propagation();
        self.focus_or_open(window, cx);
    }

    fn reveal_highlight(&mut self) {
        if let Some(flat) = self.state.highlighted_flat() {
            self.menu_scroll.scroll_to_item(flat);
        }
    }

    fn handle_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        if CommandMenuState::matches_toggle_shortcut(
            keystroke.key.as_str(),
            keystroke.modifiers.platform,
            keystroke.modifiers.control,
        ) {
            self.focus_or_open(window, cx);
            return;
        }
        match keystroke.key.as_str() {
            "escape" => {
                self.state.dismiss();
                self.focus_return(window, cx);
                cx.notify();
                return;
            }
            "down" if !keystroke.modifiers.modified() => {
                self.state.move_next();
                self.reveal_highlight();
                cx.notify();
                return;
            }
            "up" if !keystroke.modifiers.modified() => {
                self.state.move_previous();
                self.reveal_highlight();
                cx.notify();
                return;
            }
            "home" if !keystroke.modifiers.modified() => {
                self.state.move_first();
                self.reveal_highlight();
                cx.notify();
                return;
            }
            "end" if !keystroke.modifiers.modified() => {
                self.state.move_last();
                self.reveal_highlight();
                cx.notify();
                return;
            }
            "enter" if !keystroke.modifiers.modified() => {
                if self.state.activate_highlighted().is_some() {
                    self.focus_return(window, cx);
                }
                cx.notify();
                return;
            }
            _ => {}
        }
    }

    fn choose_row(
        &mut self,
        group_id: &str,
        item_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.activate_id(group_id, item_id).is_some() {
            self.focus_return(window, cx);
        }
        cx.notify();
    }

    fn hover_row(&mut self, group_id: &str, item_id: &str, _: &mut Context<Self>) {
        self.state.apply_highlight_id(group_id, item_id);
    }

    fn entry_icon(entry: &CommandMenuEntry) -> AssetId {
        match &entry.action {
            CommandMenuAction::NewThread => AssetId::TABLER_EDIT,
            CommandMenuAction::OpenSettings => AssetId::TABLER_SETTINGS,
            CommandMenuAction::OpenProject { .. } => AssetId::TABLER_FOLDER,
            CommandMenuAction::OpenThread { .. } => AssetId::TABLER_MESSAGE_CIRCLE,
        }
    }

    fn render_row(
        &self,
        group: &CommandMenuGroup,
        entry: &CommandMenuEntry,
        flat: usize,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let highlighted = self.state.highlighted_flat() == Some(flat);
        let style = ListRowStyle::resolve(
            self.theme,
            ListRowGeometry::Menu,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        );
        let selector = format!(
            "{}-{}-{}",
            COMMAND_MENU_ROW_SELECTOR_PREFIX, group.id, entry.id
        );
        let row_id = SharedString::from(format!("command-menu-row-{flat}"));
        let presentation = list_row(
            style,
            ListRowContent::one_line(SharedString::from(entry.title.clone())),
            artisan_ui::list_row::ListRowSlots::new().leading(
                icon(IconStyle::resolve(
                    self.theme,
                    Self::entry_icon(entry),
                    IconSize::Default,
                    IconTint::Muted,
                ))
                .size(px(16.0)),
            ),
        );
        let group_id = group.id.clone();
        let item_id = entry.id.clone();
        let hover_group_id = group_id.clone();
        let hover_item_id = item_id.clone();
        presentation
            .id(row_id)
            .debug_selector(move || selector.clone())
            .on_hover(cx.listener(move |view: &mut Self, hovered: &bool, _, cx| {
                if *hovered {
                    view.hover_row(&hover_group_id, &hover_item_id, cx);
                    cx.notify();
                }
            }))
            .on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.choose_row(&group_id, &item_id, window, cx);
                }),
            )
            .when(highlighted, |row| {
                row.bg(if self.anchored {
                    self.desktop_theme.selected
                } else {
                    self.theme.colors.accent.to_paint()
                })
            })
    }

    fn render_query_input(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let theme = self.theme;
        let query = SharedString::from(self.state.query().to_owned());
        let placeholder = SharedString::from(COMMAND_MENU_PLACEHOLDER);
        let input = Input::new(
            "native-command-menu-input",
            self.input_focus.clone(),
            theme,
            query,
        )
        .placeholder(placeholder)
        .focus_visibility(artisan_ui::button::FocusVisibility::Hidden)
        .debug_selector(COMMAND_MENU_INPUT_SELECTOR)
        .h(px(30.0))
        .min_w(px(0.0))
        .flex_1()
        .rounded(px(6.0))
        .border_0()
        .bg(gpui::transparent_black())
        .px(px(0.0))
        .py(px(5.0))
        .text_color(self.desktop_theme.foreground)
        .text_size(px(13.0))
        .line_height(px(18.0));
        let input = NativeCommandMenuInputElement::new(
            input.into_any_element(),
            cx.entity(),
            self.input_focus.clone(),
        );
        div()
            .relative()
            .h(px(32.0))
            .w_full()
            .min_w(px(0.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(if self.input_focus.is_focused(window) {
                self.desktop_theme.secondary
            } else {
                self.desktop_theme.field_line
            })
            .bg(self.desktop_theme.field)
            .child(
                asset_glyph(AssetId::TABLER_SEARCH)
                    .size(px(15.0))
                    .text_color(self.desktop_theme.secondary),
            )
            .child(input)
            .child(
                div()
                    .flex_shrink_0()
                    .px(px(6.0))
                    .py(px(2.0))
                    .rounded(px(4.0))
                    .border_1()
                    .border_color(self.desktop_theme.shortcut_line)
                    .text_size(px(11.0))
                    .text_color(self.desktop_theme.secondary)
                    .child("Ctrl+K"),
            )
            .into_any_element()
    }

    fn render_list(&self, cx: &Context<Self>) -> Stateful<Div> {
        let theme = self.theme;
        let mut list = div()
            .id("native-command-menu-list")
            .flex()
            .flex_col()
            .w_full()
            .min_h(px(0.0))
            .max_h(px(MENU_LIST_MAX_HEIGHT_PX))
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .debug_selector(|| COMMAND_MENU_LIST_SELECTOR.to_owned());
        let visible = self.state.visible_groups();
        if visible.is_empty() {
            list = list.child(
                div()
                    .flex()
                    .w_full()
                    .items_center()
                    .justify_center()
                    .py(px(24.0))
                    .text_size(if self.anchored {
                        px(13.0)
                    } else {
                        theme.typography.control_text
                    })
                    .text_color(if self.anchored {
                        self.desktop_theme.secondary
                    } else {
                        theme.colors.muted_foreground.to_paint()
                    })
                    .child(COMMAND_MENU_EMPTY_LABEL),
            );
        } else {
            let mut flat = 0;
            for visible_group in &visible {
                let Some(group) = self.state.groups().get(visible_group.group) else {
                    continue;
                };
                list = list.child(
                    div()
                        .w_full()
                        .px(px(8.0))
                        .py(px(6.0))
                        .text_size(if self.anchored {
                            px(11.0)
                        } else {
                            theme.typography.label_text
                        })
                        .text_color(if self.anchored {
                            self.desktop_theme.secondary
                        } else {
                            theme.colors.muted_foreground.to_paint()
                        })
                        .child(group.heading.clone()),
                );
                for row in &visible_group.rows {
                    if let Some(entry) = group.entries.get(row.index) {
                        list = list.child(self.render_row(group, entry, flat, cx));
                    }
                    flat += 1;
                }
            }
        }
        list
    }

    fn render_dialog(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let input = self.render_query_input(window, cx);
        let list = self.render_list(cx);
        let card = popover_content(
            PopoverStyle::default_card(self.theme),
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .key_context(COMMAND_MENU_KEY_CONTEXT)
                .id("native-command-menu-card-content")
                .child(input)
                .child(list)
                .on_action(cx.listener(Self::delete_backward_action))
                .on_action(cx.listener(Self::delete_forward_action))
                .on_action(cx.listener(Self::move_left_action))
                .on_action(cx.listener(Self::move_right_action))
                .on_action(cx.listener(Self::select_left_action))
                .on_action(cx.listener(Self::select_right_action))
                .on_action(cx.listener(Self::select_all))
                .on_action(cx.listener(Self::move_home))
                .on_action(cx.listener(Self::move_end))
                .on_action(cx.listener(Self::paste))
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(Self::cut))
                .on_key_down(cx.listener(Self::handle_menu_key)),
        )
        .w(px(MENU_WIDTH_PX))
        .debug_selector(|| COMMAND_MENU_SELECTOR.to_owned())
        .id("native-command-menu-card")
        .on_click(cx.listener(|_: &mut Self, _: &ClickEvent, _, cx| {
            cx.stop_propagation();
        }));
        div()
            .id("native-command-menu-scrim")
            .absolute()
            .top(px(0.0))
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .flex()
            .justify_center()
            .debug_selector(|| format!("{COMMAND_MENU_SELECTOR}-scrim"))
            .on_click(cx.listener(|view: &mut Self, _: &ClickEvent, window, cx| {
                view.state.dismiss();
                view.focus_return(window, cx);
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .w(px(MENU_WIDTH_PX))
                    .pt(px(MENU_TOP_OFFSET_PX))
                    .child(card),
            )
            .into_any_element()
    }

    fn render_titlebar(&self, window: &Window, cx: &Context<Self>) -> AnyElement {
        let input = self.render_query_input(window, cx);
        let click_entity = cx.entity();
        let mut root = div()
            .id("native-command-menu-root")
            .block_mouse_except_scroll()
            .relative()
            .w(px(360.0))
            .h(px(32.0))
            .min_w(px(0.0))
            .key_context(COMMAND_MENU_KEY_CONTEXT)
            .debug_selector(|| COMMAND_MENU_SELECTOR.to_owned())
            .on_mouse_down_all(move |event, phase, hitbox, window, app| {
                if phase == gpui::DispatchPhase::Capture
                    && event.button == gpui::MouseButton::Left
                    && hitbox.is_hovered(window)
                {
                    click_entity.update(app, |menu, cx| menu.focus_or_open(window, cx));
                }
            })
            .on_click(cx.listener(Self::handle_trigger_click))
            .on_action(cx.listener(Self::delete_backward_action))
            .on_action(cx.listener(Self::delete_forward_action))
            .on_action(cx.listener(Self::move_left_action))
            .on_action(cx.listener(Self::move_right_action))
            .on_action(cx.listener(Self::select_left_action))
            .on_action(cx.listener(Self::select_right_action))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::move_home))
            .on_action(cx.listener(Self::move_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_key_down(cx.listener(Self::handle_menu_key))
            .child(input);

        if self.state.is_open() {
            let dropdown = div()
                .id("native-command-menu-dropdown")
                .block_mouse_except_scroll()
                .absolute()
                .top(px(38.0))
                .right(px(0.0))
                .w(px(360.0))
                .max_h(px(MENU_LIST_MAX_HEIGHT_PX + 48.0))
                .flex()
                .flex_col()
                .overflow_hidden()
                .rounded(px(8.0))
                .border_1()
                .border_color(self.desktop_theme.popover_line)
                .bg(self.desktop_theme.sidebar)
                .debug_selector(|| COMMAND_MENU_DROPDOWN_SELECTOR.to_owned())
                .on_click(cx.listener(|_: &mut Self, _: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                }))
                .child(self.render_list(cx));
            root = root.child(deferred(dropdown));
        }

        root.into_any_element()
    }
}

impl Render for NativeCommandMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.anchored {
            return self.render_titlebar(window, cx);
        }
        if !self.state.is_open() {
            return div()
                .id("native-command-menu-root")
                .debug_selector(|| COMMAND_MENU_SELECTOR.to_owned())
                .into_any_element();
        }
        let dialog = self.render_dialog(window, cx);
        div()
            .id("native-command-menu-root")
            .debug_selector(|| COMMAND_MENU_SELECTOR.to_owned())
            .child(deferred(dialog))
            .into_any_element()
    }
}

/// Registers GPUI's native text service against the shared controlled Input
/// visual. The input still paints through `artisan-ui`; this wrapper only
/// supplies the editing bridge that GPUI 0.2.2 leaves to the caller.
struct NativeCommandMenuInputElement {
    child: AnyElement,
    view: Entity<NativeCommandMenu>,
    focus_handle: FocusHandle,
}

impl NativeCommandMenuInputElement {
    fn new(child: AnyElement, view: Entity<NativeCommandMenu>, focus_handle: FocusHandle) -> Self {
        Self {
            child,
            view,
            focus_handle,
        }
    }
}

impl Element for NativeCommandMenuInputElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.handle_input(
            &self.focus_handle,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        self.child.paint(window, cx);
        self.view.update(cx, |menu, _| {
            menu.input_bounds = Some(bounds);
        });
    }
}

impl IntoElement for NativeCommandMenuInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl EntityInputHandler for NativeCommandMenu {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let utf8_range = utf16_range_to_utf8(self.state.query(), range.clone())?;
        *adjusted_range = Some(range);
        Some(self.state.query()[utf8_range].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let query = self.state.query();
        Some(UTF16Selection {
            range: utf8_range_to_utf16(query, self.input_selection.clone())?,
            reversed: self.input_selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .clone()
            .and_then(|range| utf8_range_to_utf16(self.state.query(), range))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.replacement_range(range) else {
            return;
        };
        self.replace_query_range(range, text, None, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(range) = self.replacement_range(range) else {
            return;
        };
        self.replace_query_range(range, new_text, new_selected_range, cx);
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        self.input_bounds.clone()
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.input_bounds.as_ref()?;
        let offset = (f32::from(point.x) - f32::from(bounds.origin.x) - 8.0).max(0.0);
        let approximate_char = (offset / 7.0).round() as usize;
        let byte_offset = self
            .state
            .query()
            .char_indices()
            .nth(approximate_char)
            .map_or(self.state.query().len(), |(byte, _)| byte);
        utf8_offset_to_utf16(self.state.query(), byte_offset)
    }
}

fn utf8_range_to_utf16(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    Some(utf8_offset_to_utf16(text, range.start)?..utf8_offset_to_utf16(text, range.end)?)
}

fn utf8_offset_to_utf16(text: &str, offset: usize) -> Option<usize> {
    (offset <= text.len() && text.is_char_boundary(offset))
        .then(|| text[..offset].encode_utf16().count())
}

fn utf16_range_to_utf8(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    Some(utf16_offset_to_utf8(text, range.start)?..utf16_offset_to_utf8(text, range.end)?)
}

fn utf16_offset_to_utf8(text: &str, offset: usize) -> Option<usize> {
    if offset == 0 {
        return Some(0);
    }
    let mut utf16_offset = 0;
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset == offset {
            return Some(byte_offset);
        }
        utf16_offset += character.len_utf16();
        if utf16_offset == offset {
            return Some(byte_offset + character.len_utf8());
        }
        if utf16_offset > offset {
            return None;
        }
    }
    (utf16_offset == offset).then_some(text.len())
}

fn previous_character_boundary(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    text[..offset]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

fn next_character_boundary(text: &str, offset: usize) -> usize {
    text[offset..]
        .chars()
        .next()
        .map_or(offset, |character| offset + character.len_utf8())
}

/// Viewport-clamped dialog width: the preferred width, floored at zero for
/// degenerate windows.
#[must_use]
pub fn menu_width_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.width);
    px(MENU_WIDTH_PX.min(available.max(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_groups() -> Vec<CommandMenuGroup> {
        vec![
            CommandMenuGroup::actions(),
            CommandMenuGroup::new(
                "project-a",
                "Project A",
                vec![
                    CommandMenuEntry::thread("thread-1", "Ship the port", "old title one"),
                    CommandMenuEntry::thread("thread-2", "Fix the rail", "old title two"),
                ],
            ),
        ]
    }

    #[test]
    fn toggle_shortcut_matches_only_meta_or_ctrl_k() {
        assert!(CommandMenuState::matches_toggle_shortcut("k", true, false));
        assert!(CommandMenuState::matches_toggle_shortcut("k", false, true));
        assert!(!CommandMenuState::matches_toggle_shortcut(
            "k", false, false
        ));
        assert!(!CommandMenuState::matches_toggle_shortcut("j", true, false));
        assert!(!CommandMenuState::matches_toggle_shortcut(
            "kk", true, false
        ));
    }

    #[test]
    fn open_close_cycle_resets_query_and_highlight() {
        let mut state = CommandMenuState::new(fixture_groups());
        assert!(!state.is_open());
        state.open();
        assert!(state.is_open());
        state.set_query("ship");
        assert_eq!(state.row_count(), 1);
        state.dismiss();
        assert!(!state.is_open());
        assert!(state.highlighted_flat().is_none());
        state.open();
        assert_eq!(state.query(), "");
        assert_eq!(state.highlighted_flat(), Some(0));
    }

    #[test]
    fn blank_query_keeps_catalog_order_and_filters_on_text() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        assert_eq!(state.row_count(), 4);
        assert_eq!(
            state.highlighted_action(),
            Some(CommandMenuAction::NewThread)
        );
        state.set_query("ship");
        assert_eq!(state.row_count(), 1);
        assert_eq!(
            state.highlighted_action(),
            Some(CommandMenuAction::OpenThread {
                thread_id: String::from("thread-1")
            })
        );
        state.set_query("thread-2");
        assert_eq!(state.row_count(), 1);
        state.set_query("zzz-no-match");
        assert_eq!(state.row_count(), 0);
        assert!(state.highlighted_action().is_none());
    }

    #[test]
    fn keyboard_movement_wraps_and_jumps() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        assert_eq!(state.row_count(), 4);
        state.move_previous();
        assert_eq!(state.highlighted_flat(), Some(3));
        state.move_next();
        assert_eq!(state.highlighted_flat(), Some(0));
        state.move_last();
        assert_eq!(state.highlighted_flat(), Some(3));
        state.move_first();
        assert_eq!(state.highlighted_flat(), Some(0));
        state.dismiss();
        state.move_next();
        assert!(state.highlighted_flat().is_none());
    }

    #[test]
    fn activation_closes_first_and_emits_exactly_once() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        state.move_next();
        state.move_next();
        let action = state.activate_highlighted();
        assert_eq!(
            action,
            Some(CommandMenuAction::OpenThread {
                thread_id: String::from("thread-1")
            })
        );
        assert!(!state.is_open());
        assert!(state.activate_highlighted().is_none());
        assert!(state.activate_row(0).is_none());
        assert_eq!(
            state.pending_action(),
            Some(&CommandMenuAction::OpenThread {
                thread_id: String::from("thread-1")
            })
        );
        assert_eq!(
            state.take_pending_action(),
            Some(CommandMenuAction::OpenThread {
                thread_id: String::from("thread-1")
            })
        );
        assert!(state.take_pending_action().is_none());
    }

    #[test]
    fn typed_emission_resolves_stable_thread_identity() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        for character in "fix".chars() {
            state.push_query_char(character);
        }
        assert_eq!(state.query(), "fix");
        assert_eq!(
            state.activate_highlighted(),
            Some(CommandMenuAction::OpenThread {
                thread_id: String::from("thread-2")
            })
        );
        assert!(!state.pop_query_char());
    }

    #[test]
    fn project_rows_are_live_searchable_actions() {
        let mut state = CommandMenuState::new(vec![CommandMenuGroup::new(
            "projects",
            "Projects",
            vec![CommandMenuEntry::project("project-a", "Artisan")],
        )]);
        state.open();
        state.set_query("project-a");
        assert_eq!(state.row_count(), 1);
        assert_eq!(
            state.activate_highlighted(),
            Some(CommandMenuAction::OpenProject {
                project_id: String::from("project-a")
            })
        );
    }

    #[test]
    fn native_input_ranges_keep_utf16_and_utf8_boundaries_aligned() {
        let text = "A😀é";
        assert_eq!(utf8_offset_to_utf16(text, 0), Some(0));
        assert_eq!(utf8_offset_to_utf16(text, 1), Some(1));
        assert_eq!(utf8_offset_to_utf16(text, 5), Some(3));
        assert_eq!(utf8_offset_to_utf16(text, text.len()), Some(4));
        assert_eq!(utf16_offset_to_utf8(text, 2), None);
        assert_eq!(utf16_offset_to_utf8(text, 3), Some(5));
        assert_eq!(previous_character_boundary(text, 5), 1);
        assert_eq!(next_character_boundary(text, 1), 5);
    }

    #[test]
    fn disabled_refuses_open_and_discards_pending() {
        let mut state = CommandMenuState::new(fixture_groups());
        state.open();
        let _ = state.activate_highlighted();
        assert!(state.pending_action().is_some());
        state.set_disabled(true);
        assert!(!state.is_open());
        assert!(state.pending_action().is_none());
        state.set_disabled(false);
        state.press_toggle();
        assert!(state.is_open());
        state.press_toggle();
        assert!(!state.is_open());
    }
}
