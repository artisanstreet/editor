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
//! text; the `ShaderGlassSurface` material is deferred (INVENTORY §9), so the
//! menu paints a solid popover surface; and floating placement is reduced to
//! a fixed-width panel above the trigger instead of full collision machinery.

#![allow(clippy::module_name_repetitions)]

use std::mem;
use std::time::Instant;

use artisan_domain::ProjectId;
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    ClickEvent, Context, Div, FocusHandle, InteractiveElement as _, KeyDownEvent, MouseDownEvent,
    ParentElement as _, Render, SharedString, Stateful, StatefulInteractiveElement as _,
    Styled as _, Window, div, prelude::FluentBuilder as _, prelude::IntoElement, px,
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
    clock: Instant,
    last_action: Option<ProjectPickerAction>,
}

/// Painted width of the trigger row (proof geometry).
const TRIGGER_WIDTH_PX: f32 = 280.0;
/// Painted width of the open menu panel; the legacy `min(20rem, …)` clamp
/// collapses to its 320 px bound inside the fixed proof surface.
const MENU_WIDTH_PX: f32 = 320.0;
/// Gap between the trigger and the panel above it (legacy `sideOffset={10}`).
const MENU_GAP_PX: f32 = 10.0;
/// Debug selector painted on the trigger row.
const TRIGGER_SELECTOR: &str = "artisan-project-picker-trigger";
/// Debug selector painted on the open menu panel.
const MENU_SELECTOR: &str = "artisan-project-picker-menu";

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
            trigger_focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            clock: Instant::now(),
            last_action: None,
        }
    }

    /// Read-only access to the underlying interaction state.
    #[must_use]
    pub fn state(&self) -> &ProjectPickerState {
        &self.state
    }

    /// The last action this view drained and applied, if any.
    #[must_use]
    pub fn last_action(&self) -> Option<ProjectPickerAction> {
        self.last_action.clone()
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.clock.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn handle_trigger_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.press_trigger();
        self.sync_focus_after_transition(window);
        cx.notify();
    }

    fn handle_trigger_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event.keystroke.key.as_str(), "enter" | "space") {
            self.state.press_trigger();
            self.sync_focus_after_transition(window);
            cx.notify();
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
            cx.notify();
        }
    }

    fn handle_outside_press(
        &mut self,
        _: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_open() {
            self.dismiss_and_settle(window);
            cx.notify();
        }
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
    /// capture/restore policy for this leaf).
    fn sync_focus_after_transition(&mut self, window: &mut Window) {
        if self.state.is_open() {
            window.focus(&self.menu_focus);
        } else {
            window.focus(&self.trigger_focus);
        }
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

        div()
            .id("project-picker-trigger")
            .track_focus(&self.trigger_focus)
            .debug_selector(|| TRIGGER_SELECTOR.to_string())
            .when(!disabled, |row| {
                row.on_click(cx.listener(Self::handle_trigger_click))
                    .on_key_down(cx.listener(Self::handle_trigger_key))
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

    fn render_menu(&self, cx: &Context<Self>) -> Option<Stateful<Div>> {
        if !self.state.is_open() {
            return None;
        }

        let popover = self.theme.colors.popover.to_paint();
        let border = self.theme.colors.border.to_paint();

        let mut column = div()
            .id("project-picker-menu")
            .track_focus(&self.menu_focus)
            .debug_selector(|| MENU_SELECTOR.to_string())
            .on_key_down(cx.listener(Self::handle_menu_key))
            .flex()
            .flex_col()
            .absolute()
            .bottom_full()
            .left_0()
            .w(px(MENU_WIDTH_PX))
            .mb(px(MENU_GAP_PX))
            .p(px(4.0))
            .rounded(px(16.0))
            .bg(popover)
            .border_1()
            .border_color(border);

        for index in 0..self.state.projects().len() {
            column = column.child(self.render_project_row(index, cx));
        }

        // The rule that makes the last row a different kind of thing
        // (legacy hairline at 50% border alpha).
        column = column.child(
            separator(border, SeparatorAxis::Horizontal)
                .my(px(4.0))
                .mx(px(4.0)),
        );

        Some(column.child(self.render_new_project_row(cx)))
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let menu = self.render_menu(cx);
        let trigger = self.render_trigger(cx);

        div()
            .id("project-picker-root")
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
            .relative()
            .flex()
            .flex_col()
            .children(menu)
            .child(trigger)
    }
}
