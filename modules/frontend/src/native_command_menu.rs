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
//! - `CommandDialog` → a `deferred` centered overlay whose card uses the
//!   shared [`artisan_ui::popover`] recipe (`popover_content` over
//!   `PopoverStyle::default_card`); `ShaderGlassSurface` rays and the dialog
//!   enter animation have no GPUI equivalent and are documented gaps.
//! - `CommandInput` → [`artisan_ui::input::Input`] (controlled value display
//!   plus placeholder). Pinned GPUI has no editable text element, so
//!   printable keystrokes arrive through the card's key handler into
//!   [`CommandMenuState::push_query_char`]; the orchestrator owns any future
//!   `InputHandler` composition.
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
//!   plus [`CommandMenuState::matches_toggle_shortcut`]; the orchestrator owns
//!   the window-level key subscription and focus restoration.
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

use artisan_assets::AssetId;
use artisan_ui::{
    icon::{IconSize, IconStyle, IconTint, icon},
    input::Input,
    list_row::{ListRowContent, ListRowGeometry, ListRowStyle, ListRowTone, list_row},
    popover::{PopoverStyle, popover_content},
    theme::{ArtisanTheme, ThemeMode},
};
use gpui::{
    AnyElement, ClickEvent, Context, Div, FocusHandle, FontWeight, InteractiveElement as _,
    KeyDownEvent, ParentElement as _, Pixels, Render, ScrollHandle, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, deferred, div,
    prelude::{FluentBuilder as _, IntoElement},
    px,
};

use crate::command_ranking::{CommandGroup, CommandItem, filter_and_rank_groups};

/// Stable debug selector for the command-menu root.
pub const COMMAND_MENU_SELECTOR: &str = "artisan-native-command-menu";
/// Stable debug selector for the query input branch.
pub const COMMAND_MENU_INPUT_SELECTOR: &str = "artisan-native-command-menu-input";
/// Stable debug selector for the ranked list branch.
pub const COMMAND_MENU_LIST_SELECTOR: &str = "artisan-native-command-menu-list";
/// Prefix for the stable selectors painted on command rows.
pub const COMMAND_MENU_ROW_SELECTOR_PREFIX: &str = "artisan-native-command-menu-row";
/// Exact legacy input placeholder (`command-menu.svelte`).
pub const COMMAND_MENU_PLACEHOLDER: &str = "Search threads and actions…";
/// Exact legacy empty-list copy (`CommandEmpty`).
pub const COMMAND_MENU_EMPTY_LABEL: &str = "No results found.";
/// The toggle shortcut key (`meta/ctrl+k` in the legacy window handler).
pub const COMMAND_MENU_SHORTCUT_KEY: &str = "k";
/// Stable identity of the static actions group.
pub const ACTIONS_GROUP_ID: &str = "actions";
/// Exact legacy heading of the static actions group.
pub const ACTIONS_GROUP_HEADING: &str = "Actions";
/// Stable identity of the new-thread action row.
pub const NEW_THREAD_ITEM_ID: &str = "new-thread";
/// Stable identity of the open-settings action row.
pub const OPEN_SETTINGS_ITEM_ID: &str = "open-settings";
/// Exact legacy new-thread action label.
pub const NEW_THREAD_LABEL: &str = "New thread";
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
    input_focus: FocusHandle,
    menu_scroll: ScrollHandle,
}

impl NativeCommandMenu {
    /// Builds the menu over the supplied groups.
    pub fn new(groups: Vec<CommandMenuGroup>, mode: ThemeMode, cx: &mut Context<Self>) -> Self {
        Self {
            state: CommandMenuState::new(groups),
            theme: ArtisanTheme::for_mode(mode),
            input_focus: cx.focus_handle(),
            menu_scroll: ScrollHandle::new(),
        }
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
            window.focus(&self.input_focus);
        }
        cx.notify();
    }

    /// Dismisses the dialog and drops input focus back to the window.
    pub fn dismiss(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.dismiss();
        window.focus(&self.input_focus);
        cx.notify();
    }

    /// Toggles the dialog for the `Cmd/Ctrl+K` trigger contract.
    pub fn press_toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state.press_toggle();
        if self.state.is_open() {
            window.focus(&self.input_focus);
        }
        self.reveal_highlight();
        cx.notify();
    }

    /// Replaces the catalog groups (controller push path).
    pub fn replace_groups(&mut self, groups: Vec<CommandMenuGroup>, cx: &mut Context<Self>) {
        self.state.replace_groups(groups);
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

    fn reveal_highlight(&mut self) {
        if let Some(flat) = self.state.highlighted_flat() {
            self.menu_scroll.scroll_to_item(flat);
        }
    }

    fn handle_menu_key(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "escape" => {
                self.state.dismiss();
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
                self.state.activate_highlighted();
                cx.notify();
                return;
            }
            "backspace" if !keystroke.modifiers.modified() => {
                if self.state.pop_query_char() {
                    self.reveal_highlight();
                    cx.notify();
                }
                return;
            }
            _ => {}
        }
        let plain = !(keystroke.modifiers.control
            || keystroke.modifiers.alt
            || keystroke.modifiers.platform
            || keystroke.modifiers.function);
        if plain
            && let Some(typed) = keystroke
                .key_char
                .as_ref()
                .and_then(|text| text.chars().next())
                .filter(|typed| !typed.is_control())
        {
            self.state.push_query_char(typed);
            self.reveal_highlight();
            cx.notify();
        }
    }

    fn choose_row(
        &mut self,
        group_id: &str,
        item_id: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.activate_id(group_id, item_id);
        cx.notify();
    }

    fn hover_row(&mut self, group_id: &str, item_id: &str, _: &mut Context<Self>) {
        self.state.apply_highlight_id(group_id, item_id);
    }

    fn entry_icon(entry: &CommandMenuEntry) -> AssetId {
        match &entry.action {
            CommandMenuAction::NewThread => AssetId::TABLER_EDIT,
            CommandMenuAction::OpenSettings => AssetId::TABLER_SETTINGS,
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
                row.bg(self.theme.colors.accent.to_paint())
            })
    }

    fn render_dialog(&self, cx: &Context<Self>) -> AnyElement {
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
        .debug_selector(COMMAND_MENU_INPUT_SELECTOR);
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
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.muted_foreground.to_paint())
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
                        .text_size(theme.typography.label_text)
                        .text_color(theme.colors.muted_foreground.to_paint())
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
        let card = popover_content(
            PopoverStyle::default_card(theme),
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(input)
                .child(list)
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
                window.focus(&view.input_focus.clone());
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
}

impl Render for NativeCommandMenu {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.state.is_open() {
            return div()
                .id("native-command-menu-root")
                .debug_selector(|| COMMAND_MENU_SELECTOR.to_owned())
                .into_any_element();
        }
        let dialog = self.render_dialog(cx);
        div()
            .id("native-command-menu-root")
            .debug_selector(|| COMMAND_MENU_SELECTOR.to_owned())
            .child(deferred(dialog))
            .into_any_element()
    }
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
