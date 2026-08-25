//! Selection and navigation state for the native thread command list.
//!
//! Frontend-owned view-model leaf for the first-workflow thread palette
//! (INVENTORY §6.3): the ordered, grouped thread rows a renderer paints and
//! the deterministic keyboard semantics a controller forwards. Behavioral
//! evidence is the audited composition of
//! `routes/components/command-menu.svelte` (:91–104) with the pinned
//! `bits-ui@2.18.1` command engine (`dist/bits/command/command.svelte.js`),
//! as recorded in INVENTORY §6.3 :496–510:
//!
//! - `ArrowDown`/`ArrowUp` move one valid row (`updateSelectedByItem`);
//! - `Meta+ArrowDown`/`Meta+ArrowUp` jump to the last/first valid row
//!   (`#last`, `updateSelectedToIndex(0)`);
//! - `Alt+ArrowDown`/`Alt+ArrowUp` move to the next/previous rendered group
//!   and select its first valid row (`updateSelectedByGroup`), falling back
//!   to a single-row move when no neighboring group holds a valid row;
//! - `Home`/`End` select the first/last valid row;
//! - movement stops at the list edges: the palette sets no `loop` option, so
//!   `updateSelectedByItem` leaves an edge selection in place instead of
//!   wrapping;
//! - valid rows exclude disabled rows (`getValidItems`,
//!   `aria-disabled="true"`), so arrows, boundary jumps, and group moves all
//!   skip them, and a group holding only disabled rows is walked past;
//! - with no query, opening selects the first valid row and otherwise clears
//!   (`#sort` → `#selectFirstItem`);
//! - `Enter` activates the selected row by clicking it, guarded against IME
//!   composition (`isComposing || keyCode === 229`).
//!
//! Deliberately out of this leaf: filtering/ranking and the searchable item
//! values, route execution, draft creation, dialog machinery, rendering, and
//! focus plumbing. The static "Actions" rows of the palette are excluded
//! because their intents (root draft jump, settings route) belong to later
//! slices; only thread activation is evidenced here. The optional `loop`,
//! grid-column, and `vimBindings` engine features are unused by the palette
//! and are not modeled.
//!
//! Ordering is an input contract: callers supply rows already ordered as the
//! palette renders them (recency-sorted threads grouped per primary project,
//! `ProjectScopedThreadGroups` in `lib/root/thread-navigation.ts`). This
//! module never sorts, filters, or deduplicates; it consumes order verbatim
//! so renderer and model cannot disagree.

use artisan_domain::{ProjectId, ThreadId};

/// One selectable thread row in the grouped command list.
///
/// Identity is the Forge-minted [`ThreadId`] the palette keys each row on
/// (`{#each group.threads as thread (thread.thread_id)}`); `enabled` mirrors
/// the engine's `aria-disabled` validity rule rather than any present
/// callsite, which currently disables no thread rows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadRow {
    thread_id: ThreadId,
    enabled: bool,
}

impl ThreadRow {
    /// Builds an enabled row: reachable by movement and activation.
    #[must_use]
    pub fn enabled(thread_id: ThreadId) -> Self {
        Self {
            thread_id,
            enabled: true,
        }
    }

    /// Builds a disabled row: skipped by every movement and never activated.
    #[must_use]
    pub fn disabled(thread_id: ThreadId) -> Self {
        Self {
            thread_id,
            enabled: false,
        }
    }

    /// Returns the thread this row opens when activated.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns whether movement and activation may reach this row.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.enabled
    }
}

/// One rendered group of thread rows.
///
/// Mirrors the palette's keyed groups: `Some` carries the primary project a
/// section belongs to and `None` is the historical unassigned section
/// (`heading={project?.display_name ?? "Unassigned"}`). An empty or
/// all-disabled group is accepted because the engine tolerates it — such a
/// group contributes no valid rows and group movement walks past it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadListGroup {
    project_id: Option<ProjectId>,
    rows: Vec<ThreadRow>,
}

impl ThreadListGroup {
    /// Builds one group from its project identity and ordered rows.
    ///
    /// Row order is authoritative presentation order and is consumed
    /// verbatim; see the module documentation.
    #[must_use]
    pub fn new(project_id: Option<ProjectId>, rows: Vec<ThreadRow>) -> Self {
        Self { project_id, rows }
    }

    /// Returns the owning project, or `None` for the unassigned section.
    #[must_use]
    pub const fn project_id(&self) -> Option<&ProjectId> {
        self.project_id.as_ref()
    }

    /// Returns the ordered rows exactly as supplied.
    #[must_use]
    pub fn rows(&self) -> &[ThreadRow] {
        &self.rows
    }
}

/// Whether an `Enter` press arrives through an input-method composition.
///
/// The engine ignores `Enter` while composing, and additionally checks the
/// legacy `keyCode` 229 because Safari reports composed presses that way
/// (`command.svelte.js`, Enter case comment).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnterComposition {
    /// Plain Enter outside any composition: activation proceeds.
    Clear,
    /// `KeyboardEvent.isComposing` is true.
    Composing,
    /// Legacy composed-Enter report (`keyCode === 229`).
    KeyCode229,
}

impl EnterComposition {
    /// Returns whether this press must not activate anything.
    #[must_use]
    pub const fn suppresses_activation(self) -> bool {
        !matches!(self, Self::Clear)
    }
}

/// One forwarded keyboard gesture the palette's root handles.
///
/// Only the bindings INVENTORY §6.3 records for this surface appear here;
/// unsupported engine features (loop wrap, grid columns, vim bindings) have
/// no variant by construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ListKey {
    /// `ArrowDown`: one valid row down, stopping at the last.
    ArrowDown,
    /// `ArrowUp`: one valid row up, stopping at the first.
    ArrowUp,
    /// `Meta+ArrowDown`: jump to the last valid row.
    MetaArrowDown,
    /// `Meta+ArrowUp`: jump to the first valid row.
    MetaArrowUp,
    /// `Alt+ArrowDown`: first valid row of the next rendered group.
    AltArrowDown,
    /// `Alt+ArrowUp`: first valid row of the previous rendered group.
    AltArrowUp,
    /// `Home`: first valid row.
    Home,
    /// `End`: last valid row.
    End,
    /// `Enter` with its composition state: activate the selected row unless
    /// suppressed.
    Enter(EnterComposition),
}

/// What a consumed Enter asks the surrounding application to do.
///
/// Only thread activation is evidenced for this leaf: the palette renders
/// each thread row as an anchor built by `ThreadRoutePathFor`
/// (`lib/root/thread-navigation.ts`) and Enter clicks it, so executing that
/// navigation natively belongs to a later slice. Further variants arrive
/// when their surfaces do, not by speculation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadActivationIntent {
    /// Open the conversation thread the selected row represents.
    OpenThread { thread_id: ThreadId },
}

/// The selected row of one thread command list.
///
/// Selection is stored as row identity, mirroring the engine's own state:
/// every movement stores the selected item's value
/// (`setValue(item.getAttribute(COMMAND_VALUE_ATTR))`). INVENTORY §6.3
/// records no refresh/reorder contract for this surface, so this leaf makes
/// no reorder promise. For the one change the pinned engine does specify —
/// the removal of the selected item, whose teardown selects the first valid
/// row ("The item removed have been the selected one, so selection should be
/// moved to the first", `registerItem` cleanup in
/// `command.svelte.js`) — [`Self::resync`] applies the same rule.
///
/// A stored identity that does not resolve among the current valid rows
/// behaves as "nothing selected", which is the engine's exact movement
/// arithmetic when no element carries the selection (`findIndex` yields
/// `-1`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadListSelection {
    selected: Option<ThreadId>,
}

/// Flattened view of the valid rows one list exposes.
#[derive(Debug, Default)]
struct FlatRows<'a> {
    /// Valid (enabled) row identities in presentation order.
    valid_ids: Vec<&'a ThreadId>,
    /// Group index of each entry in `valid_ids`.
    group_of_valid: Vec<usize>,
    /// Per group: flattened index of its first valid row, if any.
    first_valid_of_group: Vec<Option<usize>>,
}

/// Flattens the enabled rows of one ordered list.
fn flatten(groups: &[ThreadListGroup]) -> FlatRows<'_> {
    let mut flat = FlatRows::default();
    flat.first_valid_of_group.resize(groups.len(), None);

    for (group_index, group) in groups.iter().enumerate() {
        for row in &group.rows {
            if !row.is_enabled() {
                continue;
            }
            if let Some(slot @ None) = flat.first_valid_of_group.get_mut(group_index) {
                *slot = Some(flat.valid_ids.len());
            }
            flat.valid_ids.push(&row.thread_id);
            flat.group_of_valid.push(group_index);
        }
    }

    flat
}

impl ThreadListSelection {
    /// Builds an unselected state, as before the list is opened.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies the engine's open-time selection (`#selectFirstItem`): the
    /// first valid row becomes selected, and a list with no valid rows
    /// selects nothing (the engine sets its value back to empty).
    pub fn mount(&mut self, groups: &[ThreadListGroup]) {
        let flat = flatten(groups);
        self.selected = flat.valid_ids.first().map(|id| (*id).clone());
    }

    /// Repairs selection after the surrounding projection changed rows,
    /// applying the engine's item-removal cleanup: when the selected row is
    /// gone, the first valid row becomes selected (`registerItem` teardown →
    /// `#selectFirstItem`). An identity that still resolves is left alone —
    /// the engine's cleanup acts only when the removed item was the selected
    /// one. Call this once per list refresh; between refreshes the state is
    /// deterministic on its own.
    pub fn resync(&mut self, groups: &[ThreadListGroup]) {
        let flat = flatten(groups);
        if self.selected_position(&flat).is_none() {
            self.selected = flat.valid_ids.first().map(|id| (*id).clone());
        }
    }

    /// Returns the currently selected row's thread, if it still resolves.
    #[must_use]
    pub fn selected_thread<'a>(&self, groups: &'a [ThreadListGroup]) -> Option<&'a ThreadId> {
        let flat = flatten(groups);
        let position = self.selected_position(&flat)?;
        let slot = flat.valid_ids.get(position)?;
        Some(*slot)
    }

    /// Handles one gesture, mutating selection and returning the activation
    /// intent an unsuppressed, resolvable `Enter` produces.
    ///
    /// Every other key returns `None`; `Enter` returns `None` while
    /// suppressed or when nothing valid is selected (the engine clicks
    /// nothing).
    pub fn handle_key(
        &mut self,
        groups: &[ThreadListGroup],
        key: ListKey,
    ) -> Option<ThreadActivationIntent> {
        let flat = flatten(groups);
        match key {
            ListKey::Enter(composition) => {
                if composition.suppresses_activation() {
                    return None;
                }
                let position = self.selected_position(&flat)?;
                let thread_id = (*flat.valid_ids.get(position)?).clone();
                Some(ThreadActivationIntent::OpenThread { thread_id })
            }
            ListKey::ArrowDown => {
                self.move_by_item(&flat, 1);
                None
            }
            ListKey::ArrowUp => {
                self.move_by_item(&flat, -1);
                None
            }
            ListKey::MetaArrowDown | ListKey::End => {
                self.select_last(&flat);
                None
            }
            ListKey::MetaArrowUp | ListKey::Home => {
                self.select_index(&flat, 0);
                None
            }
            ListKey::AltArrowDown => {
                self.move_by_group(&flat, 1);
                None
            }
            ListKey::AltArrowUp => {
                self.move_by_group(&flat, -1);
                None
            }
        }
    }

    /// Position of the stored selection among the valid rows, if it resolves.
    fn selected_position(&self, flat: &FlatRows<'_>) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        flat.valid_ids
            .iter()
            .position(|candidate| *candidate == selected)
    }

    /// Selects the row at one flattened valid index; out-of-range indexes do
    /// nothing (`updateSelectedToIndex` returns early past the bounds).
    fn select_index(&mut self, flat: &FlatRows<'_>, index: usize) {
        if let Some(id) = flat.valid_ids.get(index) {
            self.selected = Some((*id).clone());
        }
    }

    /// Selects the last valid row (`#last`); an empty list does nothing.
    fn select_last(&mut self, flat: &FlatRows<'_>) {
        if let Some(last) = flat.valid_ids.len().checked_sub(1) {
            self.select_index(flat, last);
        }
    }

    /// Moves relative to the current selection by one valid row
    /// (`updateSelectedByItem`, `loop` disabled).
    ///
    /// The engine locates the selection with `findIndex`, which yields `-1`
    /// for an absent selection; a downward move therefore lands on the first
    /// row while an upward move has no target. Out-of-range targets keep the
    /// current selection: edges stop, never wrap.
    fn move_by_item(&mut self, flat: &FlatRows<'_>, change: isize) {
        let current = self
            .selected_position(flat)
            .and_then(|position| isize::try_from(position).ok())
            .unwrap_or(-1);
        let Some(target) = current.checked_add(change) else {
            return;
        };
        let Ok(index) = usize::try_from(target) else {
            return;
        };
        self.select_index(flat, index);
    }

    /// Moves to the first valid row of the next/previous rendered group
    /// (`updateSelectedByGroup`).
    ///
    /// Walks sibling groups from the selection until one holds a valid row —
    /// skipping groups with none — selecting that group's first valid row.
    /// When no neighboring group qualifies, or nothing is selected, the
    /// engine falls back to a single-row move, so an edge selection stays.
    fn move_by_group(&mut self, flat: &FlatRows<'_>, change: isize) {
        let origin = self
            .selected_position(flat)
            .and_then(|position| flat.group_of_valid.get(position).copied());
        let Some(origin) = origin else {
            self.move_by_item(flat, change);
            return;
        };

        let step = isize::signum(change);
        let mut cursor = Some(origin);
        while let Some(group_index) = cursor {
            let Some(candidate) = group_index.checked_add_signed(step) else {
                break;
            };
            if candidate >= flat.first_valid_of_group.len() {
                break;
            }
            if let Some(Some(first)) = flat.first_valid_of_group.get(candidate) {
                self.select_index(flat, *first);
                return;
            }
            cursor = Some(candidate);
        }

        self.move_by_item(flat, change);
    }
}
