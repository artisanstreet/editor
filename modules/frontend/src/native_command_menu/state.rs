//! Dependency-free command menu interaction state and ranking.
//!
//! Extracted verbatim from `native_command_menu.rs` during the module split.

#[allow(clippy::wildcard_imports)]
use super::*;

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
