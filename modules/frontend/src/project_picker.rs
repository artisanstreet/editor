//! Rust-native project picker: the opening surface of the first GPUI
//! workflow.
//!
//! This is one product-specific leaf, not a shared menu framework. It
//! preserves the audited legacy behavior contract of
//! `routes/components/project-selector.svelte` (INVENTORY §6.2):
//!
//! - fully controlled open state; the trigger toggles, Escape and outside
//!   presses dismiss, and every selection closes *before* an action is
//!   emitted;
//! - an accessible trigger name (`Project: <name>` by default, caller
//!   overridable) and a disabled state that refuses to open;
//! - opening highlights the row of the *current* project rather than the
//!   first row, falling back to the first row (or "New project" with an
//!   empty catalog);
//! - arrow movement with wrap-around, Home/End jumps, and printable-prefix
//!   typeahead backed by a documented 1-second buffer (Bits'
//!   `DOMTypeahead`: repeated single characters cycle through rows starting
//!   with that character, extended prefixes scan forward from the highlight,
//!   and a missed extension restarts the buffer at the new character);
//! - Enter/Space (or pointer click) activates the highlighted row;
//!   activating the current project closes without side effect; choosing a
//!   different project emits [`ProjectPickerAction::Choose`] and "New
//!   project" — the distinct final row after a hairline separator — emits
//!   [`ProjectPickerAction::NewProject`].
//!
//! Actions are recorded explicitly in [`ProjectPickerState`] so unit tests
//! can assert the full contract without launching a window. The GPUI view is
//! deliberately small and honest about pinned-GPUI limits: there is no
//! platform accessibility tree yet, so the accessible name rides visible
//! text, and the `ShaderGlassSurface` material is deferred (INVENTORY §9), so
//! the menu paints a solid popover surface.
//!
//! Placement and traversal use the audited pinned-GPUI primitives directly:
//!
//! - the trigger is a real tab stop (its focus handle carries
//!   `tab_index(0)`/`tab_stop(true)`; pinned GPUI only syncs element-level
//!   tab settings onto implicitly generated handles), so native
//!   `Window::focus_next`/`focus_prev` traversal reaches it without pointer
//!   help; disabling removes it from traversal while keeping it focusable;
//! - the open menu mounts as a `deferred` layer so it paints above the
//!   trigger, positioned by `anchored()` in window position mode against the
//!   trigger shell's painted origin (recorded every frame by an invisible
//!   probe element): `BottomLeft` corner with a `(0, -10)` offset reproduces
//!   the legacy `side="top" align="start" sideOffset={10}` preferred
//!   placement, and pinned `SwitchAnchor` fit flips the corner (horizontal,
//!   then vertical) with every candidate staying attached to the real
//!   trigger — window mode avoids the pinned Local-mode quirk that drops
//!   the parent origin from flip candidates. Frames before the probe has
//!   recorded an origin render nothing rather than risk detached placement;
//! - the menu body is height-bounded and vertically scrollable. Known
//!   pinned-GPUI lifecycle quirks are handled rather than papered over: a
//!   fresh scroll handle's first pending item is consumed before the handle
//!   has overflow/bounds, so the shell's paint probe re-issues the reveal
//!   through `Window::defer` — queued from draw, its callback runs after
//!   that draw completes, where a refresh is legal again (`refresh` during
//!   draw is a pinned no-op) — for any highlighted row, tail or midpoint;
//!   the scroll container maps one direct child per selectable row (the
//!   hairline separator lives inside the final row group) so
//!   `ScrollHandle::scroll_to_item` indexes stay aligned with [`PickerRow`]
//!   addresses; and because the deferred menu lies outside the root hitbox,
//!   the root's outside-press handler ignores presses inside the live menu
//!   bounds so row activation wins the capture/bubble race;
//! - Enter/Space activation spans the full key lifecycle: the down half
//!   activates, closes, and restores trigger focus immediately. A release
//!   fence swallows that keypress's synthesized trigger click; a genuine
//!   later key-down or pointer click retires any stale fence.
//!
//! Behavior evidence comes from the pinned in-memory GPUI test harness
//! (real painted bounds, focus, and scroll state); it is not OS-window,
//! pixel, or platform-accessibility proof.

#![allow(clippy::module_name_repetitions)]

use std::cell::{Cell, RefCell};
use std::mem;
use std::rc::Rc;
use std::time::Instant;

use artisan_domain::ProjectId;
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    AnyElement, ClickEvent, Context, Corner, Div, FocusHandle, InteractiveElement as _,
    KeyDownEvent, MouseDownEvent, ParentElement as _, Pixels, Point, Render, ScrollHandle,
    SharedString, Size, Stateful, StatefulInteractiveElement as _, Styled as _, Window, anchored,
    canvas, deferred, div, point, prelude::FluentBuilder as _, prelude::IntoElement, px,
};

/// How long printable typeahead keeps accumulating before its buffer expires.
///
/// The documented legacy value (INVENTORY §6.2, Bits menu keyboard model):
/// one second between keystrokes; anything at or past this gap starts a
/// fresh buffer.
pub const TYPEAHEAD_BUFFER_MILLIS: u64 = 1_000;

/// Fallback subject of the default accessible trigger name.
const CHOOSE_A_PROJECT: &str = "Choose a project";
/// Label of the distinct final action row.
const NEW_PROJECT_ROW_LABEL: &str = "New project";

/// One selectable catalog entry presented by the picker.
///
/// Callers map their freshest-first catalog onto these rows; identity is the
/// Forge-minted [`ProjectId`] so chosen actions stay meaningful across
/// display-name changes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectOption {
    /// Forge-minted project identity.
    pub id: ProjectId,
    /// Display label shown in the trigger and the menu row.
    pub name: SharedString,
}

/// An action the picker requests after it has already closed itself.
///
/// Emission order is part of the contract: the picker closes first, then the
/// action becomes observable, mirroring the legacy controlled-open flow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectPickerAction {
    /// Switch the surface onto a different existing project.
    Choose(ProjectId),
    /// Open the attach-folder flow for a brand-new project.
    NewProject,
}

/// A selectable address inside the open menu: one project row or the final
/// "New project" row. The hairline separator above the final row is visual
/// only and is never addressed or highlighted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PickerRow {
    /// The attached-project row at the given catalog position.
    Project(usize),
    /// The distinct final action row.
    NewProject,
}

/// Printable-prefix accumulation buffer behind the 1-second typeahead.
#[derive(Debug, Default)]
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

/// Explicit interaction state for the project picker.
///
/// Pure and deterministic: time enters only through explicit millisecond
/// stamps, and effects leave only through [`ProjectPickerState::take_actions`],
/// so the whole contract is assertable without a window.
#[derive(Debug)]
pub struct ProjectPickerState {
    projects: Vec<ProjectOption>,
    current: Option<ProjectId>,
    trigger_label_override: Option<SharedString>,
    open: bool,
    disabled: bool,
    highlight: Option<PickerRow>,
    actions: Vec<ProjectPickerAction>,
    typeahead: TypeaheadBuffer,
}

impl ProjectPickerState {
    /// Builds picker state over a freshest-first catalog.
    ///
    /// The picker starts closed, undimmed, and carries no pending actions.
    #[must_use]
    pub fn new(projects: Vec<ProjectOption>, current: Option<ProjectId>) -> Self {
        Self {
            projects,
            current,
            trigger_label_override: None,
            open: false,
            disabled: false,
            highlight: None,
            actions: Vec::new(),
            typeahead: TypeaheadBuffer::default(),
        }
    }

    /// Sets whether the trigger refuses interaction.
    ///
    /// Disabling while a menu is open closes it immediately, without
    /// emitting any action.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
        if disabled {
            self.close_without_action();
        }
    }

    /// Returns whether the trigger currently refuses interaction.
    #[must_use]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Repoints the surface onto a different current project (the
    /// controller-side effect of a previously emitted
    /// [`ProjectPickerAction::Choose`]).
    pub fn set_current(&mut self, current: Option<ProjectId>) {
        self.current = current;
    }

    /// Returns the project the surface currently points at, if any.
    #[must_use]
    pub fn current_id(&self) -> Option<&ProjectId> {
        self.current.as_ref()
    }

    /// Overrides the whole accessible trigger name (the legacy custom-face
    /// `trigger_label`); `None` restores the derived default.
    pub fn set_trigger_label(&mut self, label: Option<SharedString>) {
        self.trigger_label_override = label;
    }

    /// Returns the presented catalog, freshest first.
    #[must_use]
    pub fn projects(&self) -> &[ProjectOption] {
        &self.projects
    }

    /// Returns whether the menu is currently open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Returns the number of selectable rows: one per project plus the final
    /// "New project" row.
    #[must_use]
    pub fn selectable_row_count(&self) -> usize {
        self.projects.len() + 1
    }

    /// Computes the trigger's accessible name.
    ///
    /// Default derivation follows the legacy row face: the current project's
    /// display name, or the invitation copy with nothing attached, prefixed
    /// with `Project:`. An explicit override (§6.2 `trigger_label`)
    /// replaces the whole name.
    #[must_use]
    pub fn trigger_label(&self) -> String {
        if let Some(label) = &self.trigger_label_override {
            return label.to_string();
        }
        let subject = self
            .current
            .as_ref()
            .and_then(|id| self.project_name(id))
            .unwrap_or(CHOOSE_A_PROJECT);
        format!("Project: {subject}")
    }

    /// Presses the trigger: toggles the controlled open state unless
    /// disabled. Opening computes the initial highlight — the current
    /// project's row when present, otherwise the first selectable row.
    pub fn press_trigger(&mut self) {
        if self.disabled {
            return;
        }
        if self.open {
            self.close_without_action();
        } else {
            self.typeahead.clear();
            self.highlight = Some(self.initial_highlight());
            self.open = true;
        }
    }

    /// Dismisses the menu (Escape or an outside press) without emitting any
    /// action.
    pub fn dismiss(&mut self) {
        self.close_without_action();
    }

    /// Returns the highlighted row while open.
    #[must_use]
    pub fn highlighted_row(&self) -> Option<PickerRow> {
        self.highlight
    }

    /// Moves the highlight down one row, wrapping past the final row.
    pub fn move_next(&mut self) {
        self.advance(true);
    }

    /// Moves the highlight up one row, wrapping past the first row.
    pub fn move_previous(&mut self) {
        self.advance(false);
    }

    /// Jumps to the first project row (Home).
    pub fn move_first(&mut self) {
        if self.open && !self.projects.is_empty() {
            self.highlight = Some(PickerRow::Project(0));
        }
    }

    /// Jumps to the final "New project" row (End).
    pub fn move_last(&mut self) {
        if self.open {
            self.highlight = Some(PickerRow::NewProject);
        }
    }

    /// Feeds one printable keystroke into the prefix typeahead at `now_ms`.
    ///
    /// Implements the documented Bits model: a buffer idle for
    /// [`TYPEAHEAD_BUFFER_MILLIS`] expires first; a repeated single character
    /// cycles through rows starting with that character; any other
    /// character extends the prefix and scans forward from the highlight
    /// (wrapping); a missed extension restarts the buffer at the new
    /// character alone.
    pub fn handle_typeahead(&mut self, input: char, now_ms: u64) {
        if !self.open || input.is_control() {
            return;
        }

        let normalized_input = input.to_lowercase().collect::<String>();

        if let Some(last_ms) = self.typeahead.last_input_ms
            && now_ms.saturating_sub(last_ms) >= TYPEAHEAD_BUFFER_MILLIS
        {
            self.typeahead.clear();
        }

        let repeated_single_char =
            self.typeahead.text.chars().count() == 1 && self.typeahead.text == normalized_input;

        let mut search = self.typeahead.text.clone();
        let matched = if repeated_single_char {
            self.find_prefix_after_highlight(&normalized_input)
        } else {
            search.push_str(&normalized_input);
            if let Some(row) = self.find_prefix_after_highlight(&search) {
                Some(row)
            } else {
                // A missed extension restarts the buffer at the new character.
                search.clear();
                search.push_str(&normalized_input);
                self.find_prefix_after_highlight(&search)
            }
        };

        self.typeahead.text = search;
        self.typeahead.last_input_ms = Some(now_ms);

        if let Some(row) = matched {
            self.highlight = Some(row);
        }
    }

    /// Returns the raw typeahead buffer (diagnostics and tests).
    #[must_use]
    pub fn typeahead_buffer(&self) -> &str {
        &self.typeahead.text
    }

    /// Activates the highlighted row with Enter/Space semantics.
    ///
    /// Closing happens strictly before any action is queued; activating the
    /// current project queues nothing at all.
    pub fn activate_highlighted(&mut self) {
        if let Some(row) = self.highlight {
            self.activate_row(row);
        }
    }

    /// Activates one specific row (pointer selection). Out-of-range rows are
    /// ignored.
    pub fn activate_row(&mut self, row: PickerRow) {
        if !self.open || !self.row_is_valid(row) {
            return;
        }
        // Close FIRST, then queue the action: the controlled-open contract.
        self.close_without_action();
        match row {
            PickerRow::Project(index) => {
                let id = self.projects[index].id.clone();
                if self.current.as_ref() != Some(&id) {
                    self.actions.push(ProjectPickerAction::Choose(id));
                }
            }
            PickerRow::NewProject => self.actions.push(ProjectPickerAction::NewProject),
        }
    }

    /// Drains every queued action in emission order.
    pub fn take_actions(&mut self) -> Vec<ProjectPickerAction> {
        mem::take(&mut self.actions)
    }

    /// Closes and resets transient menu state without emitting anything.
    fn close_without_action(&mut self) {
        self.open = false;
        self.highlight = None;
        self.typeahead.clear();
    }

    /// The initial highlight for a freshly opened menu: the current
    /// project's row wins (legacy `FocusSelectedProject`), then the first
    /// project row, then the unavoidable final row.
    fn initial_highlight(&self) -> PickerRow {
        if self.projects.is_empty() {
            return PickerRow::NewProject;
        }
        self.current
            .as_ref()
            .and_then(|id| {
                self.projects
                    .iter()
                    .position(|option| &option.id == id)
                    .map(PickerRow::Project)
            })
            .unwrap_or(PickerRow::Project(0))
    }

    fn advance(&mut self, forward: bool) {
        if !self.open {
            return;
        }
        let count = self.selectable_row_count();
        let start = self.flat_highlight();
        let next = if forward {
            if start + 1 == count { 0 } else { start + 1 }
        } else if start == 0 {
            count - 1
        } else {
            start - 1
        };
        self.highlight = Some(self.row_at_flat_index(next));
    }

    /// The highlight flattened onto the selectable-row index space; zero
    /// while closed or unset, matching the fallback scan origin.
    fn flat_highlight(&self) -> usize {
        self.highlight
            .map_or(0, |row| row.to_flat_index(self.projects.len()))
    }

    /// Restores a flattened index onto a row; indexes at or past the
    /// project-row count land on the final "New project" row.
    fn row_at_flat_index(&self, flat: usize) -> PickerRow {
        if flat >= self.projects.len() {
            PickerRow::NewProject
        } else {
            PickerRow::Project(flat)
        }
    }

    fn row_is_valid(&self, row: PickerRow) -> bool {
        match row {
            PickerRow::Project(index) => index < self.projects.len(),
            PickerRow::NewProject => true,
        }
    }

    fn project_name(&self, id: &ProjectId) -> Option<&str> {
        self.projects
            .iter()
            .find(|option| &option.id == id)
            .map(|option| option.name.as_ref())
    }

    /// Finds the first selectable row after the highlight (wrapping) whose
    /// label starts with the lowercased prefix.
    fn find_prefix_after_highlight(&self, lowercased_prefix: &str) -> Option<PickerRow> {
        let count = self.selectable_row_count();
        let start = self.flat_highlight();
        for offset in 1..=count {
            let candidate = self.row_at_flat_index((start + offset) % count.max(1));
            if self.row_matches_prefix(candidate, lowercased_prefix) {
                return Some(candidate);
            }
        }
        None
    }

    fn row_matches_prefix(&self, row: PickerRow, lowercased_prefix: &str) -> bool {
        let label = match row {
            PickerRow::Project(index) => self.projects[index].name.as_ref(),
            PickerRow::NewProject => NEW_PROJECT_ROW_LABEL,
        };
        label.to_lowercase().starts_with(lowercased_prefix)
    }
}

impl PickerRow {
    /// Flattens onto the selectable-row index space (project rows first,
    /// then the final row at position `project_count`).
    fn to_flat_index(self, project_count: usize) -> usize {
        match self {
            Self::Project(index) => index,
            Self::NewProject => project_count,
        }
    }
}

/// Native GPUI presentation of the picker leaf.
///
/// Owns a [`ProjectPickerState`], drains its actions, applies `Choose` back
/// as the controller repointing, and keeps the last applied action readable
/// for proofs and tests.
pub struct ProjectPickerView {
    state: ProjectPickerState,
    theme: ArtisanTheme,
    trigger_focus: FocusHandle,
    menu_focus: FocusHandle,
    /// Scroll state of the open menu body; one direct child per selectable
    /// row keeps `scroll_to_item` indexes aligned with [`PickerRow`].
    menu_scroll: ScrollHandle,
    /// Painted window-space origin of the trigger shell, refreshed every
    /// frame by an invisible probe element. Window-mode `anchored()` needs
    /// this absolute position; pinned-GPUI Local mode drops the shell origin
    /// in its flip candidates, which would detach a flipped menu from a
    /// trigger that is not at the window origin.
    trigger_origin: Rc<RefCell<Option<Point<Pixels>>>>,
    /// Flat selectable-row address waiting to be revealed once the fresh
    /// scroll handle has received real bounds. Pinned-GPUI consumes a
    /// pending scroll item before a brand-new handle has overflow/bounds,
    /// so the shell's paint-time probe re-issues the reveal request after
    /// that first draw completes, via a lifecycle-deferred refresh that is
    /// legal outside the suppressed drawing phase.
    initial_reveal_flat: Rc<Cell<Option<usize>>>,
    /// Set while the release half of a menu-closing Enter/Space keypress is
    /// outstanding. That release synthesizes a focused-trigger click (pinned
    /// div.rs key-up synthesis); exactly this one click is swallowed so the
    /// menu closes and stays closed through the full press lifecycle.
    suppress_trigger_release: bool,
    clock: Instant,
    last_action: Option<ProjectPickerAction>,
}

/// Painted width of the trigger row (proof geometry).
const TRIGGER_WIDTH_PX: f32 = 280.0;
/// Preferred painted width of the open menu panel; the legacy
/// `min(20rem, calc(100vw - 2rem))` clamp bound.
const MENU_WIDTH_PX: f32 = 320.0;
/// Horizontal viewport inset the legacy width clamp reserves (`2rem` total),
/// so the panel never touches a window edge even when the preferred width
/// cannot fit.
const MENU_VIEWPORT_INSET_X_PX: f32 = 32.0;
/// Bounded maximum menu body height. The legacy surface had no catalog large
/// enough to overflow its window; this leaf deliberately bounds the panel and
/// scrolls instead of squeezing long catalogs or overflowing the window.
const MENU_MAX_HEIGHT_PX: f32 = 360.0;
/// Vertical viewport inset reserved when bounding the menu height, mirroring
/// the horizontal clamp so the bounded panel always fits with breathing room.
const MENU_VIEWPORT_INSET_Y_PX: f32 = 32.0;
/// Gap between the trigger and the panel above it (legacy `sideOffset={10}`).
/// Applied only to the preferred placement; pinned-GPUI flip candidates use
/// the raw anchor origin without the offset.
const MENU_GAP_PX: f32 = 10.0;
/// Debug selector painted on the trigger row.
const TRIGGER_SELECTOR: &str = "artisan-project-picker-trigger";
/// Debug selector painted on the open menu panel.
const MENU_SELECTOR: &str = "artisan-project-picker-menu";
/// Prefix of the debug selectors painted on selectable rows: project rows are
/// suffixed with their catalog index, the final row with `-new`.
const ROW_SELECTOR_PREFIX: &str = "artisan-project-picker-row";

/// Menu panel width for a viewport: the legacy `min(20rem, 100vw - 2rem)`
/// clamp, floored at zero for degenerate windows.
fn menu_width_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.width) - MENU_VIEWPORT_INSET_X_PX;
    px(MENU_WIDTH_PX.min(available.max(0.0)))
}

/// Bounded menu body height for a viewport: the leaf maximum, clamped so the
/// panel plus the reserved inset always fit inside the window.
fn menu_max_height_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.height) - MENU_VIEWPORT_INSET_Y_PX;
    px(MENU_MAX_HEIGHT_PX.min(available.max(0.0)))
}

impl ProjectPickerView {
    /// Builds the view over an initial catalog and current project.
    pub fn new(
        projects: Vec<ProjectOption>,
        current: Option<ProjectId>,
        mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: ProjectPickerState::new(projects, current),
            theme: ArtisanTheme::for_mode(mode),
            // Pinned GPUI only applies element-level `tab_index`/`tab_stop`
            // to implicitly generated handles; an explicitly tracked handle
            // must carry its own tab settings, or paint inserts it into the
            // stop map with `tab_stop: false` and traversal skips it forever.
            trigger_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            menu_focus: cx.focus_handle(),
            menu_scroll: ScrollHandle::new(),
            trigger_origin: Rc::new(RefCell::new(None)),
            initial_reveal_flat: Rc::new(Cell::new(None)),
            suppress_trigger_release: false,
            clock: Instant::now(),
            last_action: None,
        }
    }

    /// Read-only access to the underlying interaction state.
    #[must_use]
    pub fn state(&self) -> &ProjectPickerState {
        &self.state
    }

    /// The trigger's focus handle: the tab stop a real traversal must land on
    /// while the menu is closed, and the restoration target after dismissal.
    #[must_use]
    pub fn trigger_focus(&self) -> &FocusHandle {
        &self.trigger_focus
    }

    /// The open menu panel's focus handle.
    #[must_use]
    pub fn menu_focus(&self) -> &FocusHandle {
        &self.menu_focus
    }

    /// The last action this view drained and applied, if any.
    #[must_use]
    pub fn last_action(&self) -> Option<ProjectPickerAction> {
        self.last_action.clone()
    }

    /// Sets whether the trigger refuses interaction.
    ///
    /// Disabling while a menu is open closes it immediately, without
    /// emitting any action. This minimal view seam lets real controllers
    /// (and native probes) drive the leaf the same way they drive its state
    /// model.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.state.set_disabled(disabled);
        if disabled {
            self.initial_reveal_flat.set(None);
        }
        cx.notify();
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn handle_trigger_click(
        &mut self,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Swallow exactly the synthesized keyboard click of a release whose
        // down half just closed the menu. Pointer clicks clear any stale
        // suppression and behave normally.
        if matches!(event, ClickEvent::Keyboard(_)) && self.suppress_trigger_release {
            self.suppress_trigger_release = false;
            return;
        }
        self.suppress_trigger_release = false;
        self.state.press_trigger();
        self.sync_focus_after_transition(window);
        cx.notify();
    }

    /// Retires a stale release-suppression fence at the first genuine
    /// (non-auto-repeat) key-down on the refocused trigger. Pinned GPUI
    /// synthesizes clicks only for unmodified Enter/Space releases; a
    /// modified release (e.g. ctrl-enter) can leave the fence armed without
    /// synthesizing a click. Without this retirement it would swallow the next
    /// legitimate opening click. `is_held` keeps held-key autorepeats from
    /// being mistaken for a new interaction.
    fn disarm_stale_release_fence(
        &mut self,
        event: &KeyDownEvent,
        _: &mut Window,
        _: &mut Context<Self>,
    ) {
        if !event.is_held {
            self.suppress_trigger_release = false;
        }
    }

    fn handle_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let mut handled = match keystroke.key.as_str() {
            "down" => {
                self.state.move_next();
                true
            }
            "up" => {
                self.state.move_previous();
                true
            }
            "home" => {
                self.state.move_first();
                true
            }
            "end" => {
                self.state.move_last();
                true
            }
            "enter" | "space" => {
                self.state.activate_highlighted();
                self.drain_actions();
                self.sync_focus_after_transition(window);
                // Unconditionally fence this closing press: pinned GPUI
                // decides whether to synthesize a focused-trigger click from
                // the ACTUAL key-up modifiers (Ctrl may be released first),
                // so arming cannot be decided from the down half alone. A
                // fence left stale by a modified release is retired by the
                // trigger's genuine-key-down disarm below, and pointer
                // interaction clears it directly.
                self.suppress_trigger_release = true;
                true
            }
            "escape" => {
                self.dismiss_and_settle(window);
                true
            }
            _ => false,
        };

        // Typeahead only consumes unmodified printable input, like Bits.
        let plain = !(keystroke.modifiers.control
            || keystroke.modifiers.alt
            || keystroke.modifiers.platform
            || keystroke.modifiers.function);
        if !handled
            && plain
            && let Some(typed) = keystroke
                .key_char
                .as_ref()
                .and_then(|text| text.chars().next())
                .filter(|typed| !typed.is_control())
        {
            self.state.handle_typeahead(typed, self.now_ms());
            handled = true;
        }

        if handled {
            self.reveal_highlight();
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
        // The deferred menu paints outside the root hitbox, and this capture
        // handler fires whenever the press misses the root — including
        // presses on menu rows. A press inside the live menu bounds belongs
        // to row activation and must win; everything else dismisses.
        if self.menu_scroll.bounds().contains(&event.position) {
            return;
        }
        self.dismiss_and_settle(window);
        cx.notify();
    }

    fn choose_row(&mut self, row: PickerRow, window: &mut Window, cx: &mut Context<Self>) {
        self.state.activate_row(row);
        self.drain_actions();
        self.sync_focus_after_transition(window);
        cx.notify();
    }

    fn dismiss_and_settle(&mut self, window: &mut Window) {
        self.state.dismiss();
        window.focus(&self.trigger_focus);
    }

    /// Refocuses whichever surface owns the keyboard after an open/close
    /// transition: the menu while open, the trigger once closed (minimal
    /// capture/restore policy for this leaf). Opening also arms a first-open
    /// reveal of the initial highlight, which may sit far below the fold of
    /// a long catalog.
    fn sync_focus_after_transition(&mut self, window: &mut Window) {
        if self.state.is_open() {
            window.focus(&self.menu_focus);
            // Immediate attempt: harmless once the handle has bounds from an
            // earlier open. A brand-new handle consumes this first pending
            // item before it has overflow/bounds and silently drops it, so
            // the flat address is also armed for the shell's paint-time
            // probe, which re-issues it after that first draw completes.
            let project_count = self.state.projects().len();
            let flat = self
                .state
                .highlighted_row()
                .map_or(0, |row| row.to_flat_index(project_count));
            self.initial_reveal_flat.set(Some(flat));
            self.reveal_highlight();
        } else {
            self.initial_reveal_flat.set(None);
            window.focus(&self.trigger_focus);
        }
    }

    /// Scrolls the menu body minimally so the highlighted row is inside the
    /// bounded panel. The scroll container maps one direct child per
    /// selectable row, so the flat row address doubles as the child index.
    /// Pending scroll targets are consumed at prepaint; calling this while
    /// closed or after a close is a no-op.
    fn reveal_highlight(&mut self) {
        if !self.state.is_open() {
            return;
        }
        let project_count = self.state.projects().len();
        let flat = self
            .state
            .highlighted_row()
            .map_or(0, |row| row.to_flat_index(project_count));
        self.menu_scroll.scroll_to_item(flat);
    }

    /// Applies drained actions: `Choose` repoints the surface (this view
    /// plays the controller until the real one lands), and everything is
    /// recorded for observation.
    fn drain_actions(&mut self) {
        for action in self.state.take_actions() {
            match action {
                ProjectPickerAction::Choose(id) => {
                    self.last_action = Some(ProjectPickerAction::Choose(id.clone()));
                    self.state.set_current(Some(id));
                }
                new_project @ ProjectPickerAction::NewProject => {
                    self.last_action = Some(new_project);
                }
            }
        }
    }

    fn render_trigger(&self, cx: &Context<Self>) -> Stateful<Div> {
        let foreground = self.theme.colors.foreground.to_paint();
        let muted_foreground = self.theme.colors.muted_foreground.to_paint();
        let surface = self.theme.colors.muted.to_paint();
        let disabled = self.state.is_disabled();

        // A disabled trigger keeps its tab-index order position but leaves
        // traversal, matching its refusal to open; the write goes through
        // the shared FocusMap entry the paint-time stop insertion reads.
        self.trigger_focus.clone().tab_stop(!disabled);

        div()
            .id("project-picker-trigger")
            .track_focus(&self.trigger_focus)
            .debug_selector(|| TRIGGER_SELECTOR.to_string())
            // A fresh (non-auto-repeat) key-down on the re-focused trigger is
            // a genuine new keyboard interaction: it retires any stale
            // release fence left behind by a previous closing press.
            .on_key_down(cx.listener(Self::disarm_stale_release_fence))
            // Keyboard activation rides GPUI's synthesized unmodified
            // Enter/Space clicks for the focused element (the same channel
            // the audited Button uses), so pointer and keyboard share one
            // toggle path and cannot double-fire.
            .when(!disabled, |row| {
                row.on_click(cx.listener(Self::handle_trigger_click))
            })
            .flex()
            .items_center()
            .gap(px(10.0))
            .w(px(TRIGGER_WIDTH_PX))
            .px(px(8.0))
            .py(px(8.0))
            .rounded(px(8.0))
            .bg(surface)
            .when(disabled, |row| row.opacity(0.5))
            .child(self.identity_dot())
            .child(
                div()
                    .flex_1()
                    .text_size(px(14.0))
                    .text_color(foreground)
                    .child(self.state.trigger_label()),
            )
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(muted_foreground)
                    .child("▾"),
            )
    }

    /// Renders the open menu as a deferred `anchored()` layer in window
    /// position mode against the captured trigger origin: preferred
    /// `BottomLeft` placement with the legacy `(0, -10)` top/start
    /// separation, pinned `SwitchAnchor` flip (horizontal, then vertical)
    /// for collision — which in window mode keeps every candidate attached
    /// to the real trigger — and a height-bounded vertically scrollable
    /// body. The hairline separator is folded into the final row group so
    /// every direct child of the scroll container addresses exactly one
    /// selectable row. Frames before the probe has recorded the trigger
    /// origin render nothing rather than risk detached placement.
    fn render_menu(&self, viewport: Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.state.is_open() {
            return None;
        }
        let trigger_origin = self.trigger_origin.borrow().as_ref().copied()?;

        let popover = self.theme.colors.popover.to_paint();
        let border = self.theme.colors.border.to_paint();

        let mut body = div()
            .id("project-picker-menu")
            .track_focus(&self.menu_focus)
            .debug_selector(|| MENU_SELECTOR.to_string())
            .on_key_down(cx.listener(Self::handle_menu_key))
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .w(menu_width_for_viewport(viewport))
            .max_h(menu_max_height_for_viewport(viewport))
            .p(px(4.0))
            .rounded(px(16.0))
            .bg(popover)
            .border_1()
            .border_color(border);

        for index in 0..self.state.projects().len() {
            body = body.child(self.render_project_row(index, cx));
        }

        // The rule that makes the last row a different kind of thing (legacy
        // hairline at 50% border alpha), grouped with the final row so the
        // scroll container keeps one child per selectable address.
        body = body.child(
            div()
                .flex()
                .flex_col()
                .child(
                    separator(border, SeparatorAxis::Horizontal)
                        .my(px(4.0))
                        .mx(px(4.0)),
                )
                .child(self.render_new_project_row(cx)),
        );

        Some(
            anchored()
                .anchor(Corner::BottomLeft)
                .position(trigger_origin)
                .offset(point(px(0.0), px(-MENU_GAP_PX)))
                .child(body)
                .into_any_element(),
        )
    }

    fn render_project_row(&self, index: usize, cx: &Context<Self>) -> Stateful<Div> {
        let option = &self.state.projects()[index];
        let selected = self
            .state
            .current_id()
            .is_some_and(|current| current == &option.id);
        let foreground = self.theme.colors.foreground.to_paint();

        self.render_selectable_row(PickerRow::Project(index), cx)
            .gap(px(10.0))
            .child(self.identity_dot())
            .child(
                div()
                    .flex_1()
                    .text_size(px(14.0))
                    .text_color(foreground)
                    .child(option.name.clone()),
            )
            .when(selected, |entry| {
                entry.child(
                    div()
                        .text_size(px(16.0))
                        .text_color(self.theme.colors.muted_foreground.to_paint())
                        .child("✓"),
                )
            })
    }

    fn render_new_project_row(&self, cx: &Context<Self>) -> Stateful<Div> {
        self.render_selectable_row(PickerRow::NewProject, cx)
            .gap(px(10.0))
            .child(
                div()
                    .size(px(24.0))
                    .rounded(px(4.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child("+"),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(14.0))
                    .text_color(self.theme.colors.foreground.to_paint())
                    .child(NEW_PROJECT_ROW_LABEL),
            )
    }

    fn render_selectable_row(&self, row: PickerRow, cx: &Context<Self>) -> Stateful<Div> {
        let highlighted = self.state.highlighted_row() == Some(row);
        let selector = match row {
            PickerRow::Project(index) => format!("{ROW_SELECTOR_PREFIX}-{index}"),
            PickerRow::NewProject => format!("{ROW_SELECTOR_PREFIX}-new"),
        };
        div()
            .id(match row {
                PickerRow::Project(index) => SharedString::from(format!("project-row-{index}")),
                PickerRow::NewProject => SharedString::from("project-row-new"),
            })
            .on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, context| {
                    view.choose_row(row, window, context);
                }),
            )
            .flex()
            .items_center()
            .w_full()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(12.0))
            .debug_selector(move || selector.clone())
            .when(highlighted, |entry| {
                entry.bg(self.theme.colors.accent.to_paint())
            })
    }

    fn identity_dot(&self) -> Div {
        div()
            .size(px(8.0))
            .flex_shrink_0()
            .rounded_full()
            .bg(self.theme.colors.primary.to_paint())
    }
}

impl Render for ProjectPickerView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let menu = self.render_menu(viewport, cx);
        let trigger = self.render_trigger(cx);

        // Invisible probe overlaying the trigger shell records its painted
        // window-space origin every frame — pinned flip candidates drop
        // contextual origins, so the anchored menu reads the last recorded
        // value to stay trigger-relative. Because this closure runs during
        // draw, any follow-up work is queued through `Window::defer`, whose
        // callback executes after the current effect cycle's draw completes:
        // `refresh` called directly here would be a pinned no-op
        // (`invalidator.not_drawing()` is false mid-draw), and the test
        // platform never pumps frame callbacks at all. The deferred reveal
        // re-issues the fresh handle's dropped pending item once real
        // geometry exists, and the deferred refresh settles one frame after
        // a host move/resize so the open menu follows the trigger.
        let probe_origin = Rc::clone(&self.trigger_origin);
        let probe_reveal = Rc::clone(&self.initial_reveal_flat);
        let probe_scroll = self.menu_scroll.clone();
        let probe = canvas(
            {
                // Prepaint deliberately records nothing: the paint closure
                // below both records and compares, so a host move detected
                // here triggers exactly one settlement frame.
                move |_, _, _| {}
            },
            {
                let probe_origin = Rc::clone(&probe_origin);
                move |bounds, (), window, cx| {
                    let moved = *probe_origin.borrow_mut() != Some(bounds.origin);
                    *probe_origin.borrow_mut() = Some(bounds.origin);
                    if let Some(flat) = probe_reveal.take() {
                        let scroll = probe_scroll.clone();
                        window.defer(cx, move |window, _| {
                            scroll.scroll_to_item(flat);
                            window.refresh();
                        });
                    } else if moved {
                        window.defer(cx, |window, _| window.refresh());
                    }
                }
            },
        )
        .absolute()
        .size_full();

        // The leaf root owns the tab group that scopes the trigger's native
        // traversal order and the outside-press dismissal boundary; the
        // shell hugs the trigger so the probe's bounds are exactly the
        // shell's, and the deferred menu paints above both.
        div()
            .id("project-picker-root")
            .tab_group()
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(probe)
                    .children(menu.map(deferred))
                    .child(trigger),
            )
    }
}
