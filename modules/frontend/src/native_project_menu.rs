//! Native GPUI project menu for the rail (project selector surface).
//!
//! Native counterpart of the menu half of
//! `routes/components/project-selector.svelte`, minus the Effect identity
//! controller, hover-pill machinery, and `ShaderGlassSurface` material, which
//! stay with the orchestrator. The legacy trigger is a quiet row (identity
//! mark, name, chevron) whose menu opens upward off its leading edge; the
//! menu lists the attached catalog freshest-first with compact mono paths, a
//! hairline separator, and a distinct final `New project` row. This surface
//! additionally carries the `Machine: This computer` row named by the rail
//! contract, so a surface with no attached project can still point at the
//! local machine.
//!
//! [`ProjectMenuState`] is the dependency-free interaction contract:
//! controlled open state, trigger label derivation, arrow/Home/End movement
//! with wrap-around, and single-emission selection events. [`NativeProjectMenu`]
//! is the thin GPUI view over that state, following the audited
//! [`crate::project_picker`] placement pattern (`deferred` + `anchored`
//! against a probe-recorded trigger origin, legacy `side="top"`
//! `align="start"` `sideOffset={10}`).
//!
//! Fidelity mapping (legacy element → this module, Tailwind → Styled notes):
//!
//! - `DropdownMenuTrigger` row → a muted-surface row (`px-2 py-2`,
//!   `rounded-lg`): identity dot, truncating label, chevron glyph
//!   (`TABLER_SELECTOR`, the Tabler `selector` mark the legacy trigger
//!   paints). The `focus-visible:ring` treatment has no GPUI equivalent;
//!   every node keeps a stable debug selector instead.
//! - `DropdownMenuContent` (`w-[min(20rem,calc(100vw-2rem))]`, `rounded-2xl`)
//!   → deferred anchored panel, 320 px preferred width clamped to the
//!   viewport, 16 px corners, popover fill, hairline border.
//! - Project rows (`rounded-xl px-2 py-1.5`, name plus compact mono path,
//!   chosen `Check`) → shared [`artisan_ui::list_row`] menu recipe with a
//!   leading identity dot and a trailing `✓` check on the current row; the
//!   recipe's `--radius-xl` corners match the legacy `rounded-xl` exactly.
//! - `Machine: This computer` row → a one-line menu row with the
//!   `TABLER_DEVICE_LAPTOP` leading mark and its own check while selected.
//! - Hairline separator → [`artisan_ui::separator`] (horizontal).
//! - `New project` row (`FolderPlus` mark) → a menu row with the
//!   `TABLER_FOLDER_PLUS` leading mark above the same hairline.
//!
//! Deliberately absent: [`artisan_ui::button`] and [`artisan_ui::badge`]
//! have no counterpart in the legacy selector (its rows are menu items, not
//! buttons or badges); the typeahead contract stays owned by
//! [`crate::project_picker`], whose fuller keyboard model this narrower rail
//! surface intentionally does not duplicate.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{cell::RefCell, rc::Rc};

use artisan_assets::AssetId;
use artisan_ui::{
    icon::{IconSize, IconStyle, IconTint, icon},
    list_row::{
        ListRowContent, ListRowGeometry, ListRowSlots, ListRowStyle, ListRowTone, list_row,
    },
    separator::{SeparatorAxis, separator},
    theme::{ArtisanTheme, ThemeMode},
};
use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, Div, FocusHandle, FontWeight,
    InteractiveElement as _, KeyDownEvent, MouseDownEvent, ParentElement as _, Pixels, Point,
    Render, ScrollHandle, SharedString, Size, Stateful, StatefulInteractiveElement as _,
    Styled as _, Window, anchored, canvas, deferred, div, point,
    prelude::{FluentBuilder as _, IntoElement},
    px,
};

/// Stable debug selector for the project-menu trigger row.
pub const PROJECT_MENU_TRIGGER_SELECTOR: &str = "artisan-native-project-menu-trigger";
/// Stable debug selector for the open project-menu panel.
pub const PROJECT_MENU_SELECTOR: &str = "artisan-native-project-menu";
/// Prefix for the stable selectors painted on project-menu rows.
pub const PROJECT_MENU_ROW_SELECTOR_PREFIX: &str = "artisan-native-project-menu-row";
/// Invitation copy with nothing attached (exact legacy label).
pub const CHOOSE_A_PROJECT: &str = "Choose a project";
/// Machine row subject copy.
pub const THIS_COMPUTER_LABEL: &str = "This computer";
/// Exact machine row title.
pub const MACHINE_ROW_TITLE: &str = "Machine: This computer";
/// Exact final action row label (legacy `New project`).
pub const NEW_PROJECT_ROW_LABEL: &str = "New project";

/// Preferred panel width (legacy `min(20rem, calc(100vw - 2rem))`).
const MENU_WIDTH_PX: f32 = 320.0;
/// Horizontal viewport inset reserved by the legacy width clamp.
const MENU_VIEWPORT_INSET_X_PX: f32 = 32.0;
/// Bounded panel height so long catalogs scroll instead of overflowing.
const MENU_MAX_HEIGHT_PX: f32 = 360.0;
/// Vertical viewport inset reserved when bounding the panel height.
const MENU_VIEWPORT_INSET_Y_PX: f32 = 32.0;
/// Gap between the trigger and the panel above it (legacy `sideOffset={10}`).
const MENU_GAP_PX: f32 = 10.0;
/// Trigger row width.
const TRIGGER_WIDTH_PX: f32 = 280.0;

/// One attached project presented by the menu.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectMenuOption {
    /// Forge-minted project identity.
    pub id: String,
    /// Display name shown in the trigger and the menu row.
    pub name: String,
    /// Compact filesystem path shown under the name, when known.
    pub path: Option<String>,
}

impl ProjectMenuOption {
    /// Builds an option with no compact path.
    #[must_use]
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            path: None,
        }
    }

    /// Supplies the compact path rendered under the name.
    #[must_use]
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

/// What the surface currently points at.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectMenuCurrent {
    /// One attached project, by Forge-minted identity.
    Project(String),
    /// The local machine (no attached project needed).
    Machine,
}

/// A selectable address inside the open menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectMenuRow {
    /// The attached-project row at the given catalog position.
    Project(usize),
    /// The `Machine: This computer` row.
    Machine,
    /// The distinct final `New project` row.
    NewProject,
}

/// One selection event emitted after the menu has already closed itself.
///
/// Emission order is part of the contract: the menu closes first, then the
/// event becomes observable, mirroring the legacy controlled-open flow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectMenuEvent {
    /// Point the surface at a different attached project.
    ChooseProject {
        /// Forge-minted project identity.
        project_id: String,
    },
    /// Point the surface at the local machine.
    ChooseMachine,
    /// Open the attach-folder flow for a brand-new project.
    NewProject,
}

/// Explicit interaction state for the project menu.
///
/// Pure and deterministic: effects leave only through
/// [`Self::take_pending_event`], so the whole contract is assertable without
/// a window.
#[derive(Debug)]
pub struct ProjectMenuState {
    options: Vec<ProjectMenuOption>,
    current: Option<ProjectMenuCurrent>,
    open: bool,
    disabled: bool,
    highlight: Option<ProjectMenuRow>,
    pending: Option<ProjectMenuEvent>,
}

impl ProjectMenuState {
    /// Builds menu state over a freshest-first catalog and current target.
    #[must_use]
    pub fn new(options: Vec<ProjectMenuOption>, current: Option<ProjectMenuCurrent>) -> Self {
        Self {
            options,
            current,
            open: false,
            disabled: false,
            highlight: None,
            pending: None,
        }
    }

    /// Returns the presented catalog, freshest first.
    #[must_use]
    pub fn options(&self) -> &[ProjectMenuOption] {
        &self.options
    }

    /// Returns what the surface currently points at, if anything.
    #[must_use]
    pub fn current(&self) -> Option<&ProjectMenuCurrent> {
        self.current.as_ref()
    }

    /// Repoints the surface (the controller-side effect of a previously
    /// emitted [`ProjectMenuEvent::ChooseProject`]/`ChooseMachine`).
    pub fn set_current(&mut self, current: Option<ProjectMenuCurrent>) {
        self.current = current;
    }

    /// Sets whether the trigger refuses interaction. Disabling closes an
    /// open menu and discards the one event waiting to be consumed.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
        if disabled {
            self.close_without_action();
            self.pending = None;
        }
    }

    /// Returns whether the trigger refuses interaction.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns whether the menu is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Returns the number of selectable rows: one per project, plus the
    /// machine row and the final new-project row.
    #[must_use]
    pub fn selectable_row_count(&self) -> usize {
        self.options.len() + 2
    }

    /// Computes the trigger's accessible name.
    ///
    /// The current project's display name wins, then the machine row title,
    /// then the invitation copy; the whole subject is prefixed with
    /// `Project:` exactly like the legacy `aria-label`.
    #[must_use]
    pub fn trigger_label(&self) -> String {
        let subject = match &self.current {
            Some(ProjectMenuCurrent::Project(id)) => self
                .options
                .iter()
                .find(|option| &option.id == id)
                .map_or(CHOOSE_A_PROJECT, |option| option.name.as_str()),
            Some(ProjectMenuCurrent::Machine) => MACHINE_ROW_TITLE,
            None => CHOOSE_A_PROJECT,
        };
        format!("Project: {subject}")
    }

    /// Presses the trigger: toggles the controlled open state unless
    /// disabled or an event is still waiting to be consumed. Opening
    /// highlights the current target's row when present, else the first
    /// project row (or the machine row with an empty catalog).
    pub fn press_trigger(&mut self) {
        if self.disabled || self.pending.is_some() {
            return;
        }
        if self.open {
            self.close_without_action();
        } else {
            self.highlight = Some(self.initial_highlight());
            self.open = true;
        }
    }

    /// Dismisses the menu (Escape or an outside press) without emitting.
    pub fn dismiss(&mut self) {
        self.close_without_action();
    }

    /// Returns the highlighted row while open.
    #[must_use]
    pub fn highlighted_row(&self) -> Option<ProjectMenuRow> {
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
        if self.open {
            self.highlight = Some(self.row_at_flat_index(0));
        }
    }

    /// Jumps to the final new-project row (End).
    pub fn move_last(&mut self) {
        if self.open {
            self.highlight = Some(ProjectMenuRow::NewProject);
        }
    }

    /// Activates the highlighted row with Enter/Space semantics.
    ///
    /// Closing happens strictly before any event is queued; activating the
    /// current target queues nothing at all.
    pub fn activate_highlighted(&mut self) {
        if let Some(row) = self.highlight {
            self.activate_row(row);
        }
    }

    /// Activates one specific row (pointer selection). Out-of-range rows are
    /// ignored, and at most one event waits unconsumed (single emission).
    pub fn activate_row(&mut self, row: ProjectMenuRow) {
        if !self.open || self.pending.is_some() || !self.row_is_valid(row) {
            return;
        }
        self.close_without_action();
        match row {
            ProjectMenuRow::Project(index) => {
                let id = self.options[index].id.clone();
                let already_current =
                    self.current.as_ref() == Some(&ProjectMenuCurrent::Project(id.clone()));
                if !already_current {
                    self.pending = Some(ProjectMenuEvent::ChooseProject { project_id: id });
                }
            }
            ProjectMenuRow::Machine => {
                if self.current.as_ref() != Some(&ProjectMenuCurrent::Machine) {
                    self.pending = Some(ProjectMenuEvent::ChooseMachine);
                }
            }
            ProjectMenuRow::NewProject => {
                self.pending = Some(ProjectMenuEvent::NewProject);
            }
        }
    }

    /// Returns and clears the one event waiting for orchestrator observation.
    pub fn take_pending_event(&mut self) -> Option<ProjectMenuEvent> {
        self.pending.take()
    }

    /// Returns the pending event without consuming it.
    #[must_use]
    pub fn pending_event(&self) -> Option<&ProjectMenuEvent> {
        self.pending.as_ref()
    }

    fn close_without_action(&mut self) {
        self.open = false;
        self.highlight = None;
    }

    fn initial_highlight(&self) -> ProjectMenuRow {
        match &self.current {
            Some(ProjectMenuCurrent::Project(id)) => self
                .options
                .iter()
                .position(|option| &option.id == id)
                .map_or(self.row_at_flat_index(0), ProjectMenuRow::Project),
            Some(ProjectMenuCurrent::Machine) => ProjectMenuRow::Machine,
            None => self.row_at_flat_index(0),
        }
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

    fn flat_highlight(&self) -> usize {
        self.highlight.map_or(0, |row| self.flat_index(row))
    }

    fn flat_index(&self, row: ProjectMenuRow) -> usize {
        match row {
            ProjectMenuRow::Project(index) => index.min(self.options.len()),
            ProjectMenuRow::Machine => self.options.len(),
            ProjectMenuRow::NewProject => self.options.len() + 1,
        }
    }

    fn row_at_flat_index(&self, flat: usize) -> ProjectMenuRow {
        match flat.cmp(&self.options.len()) {
            std::cmp::Ordering::Less => ProjectMenuRow::Project(flat),
            std::cmp::Ordering::Equal => ProjectMenuRow::Machine,
            std::cmp::Ordering::Greater => ProjectMenuRow::NewProject,
        }
    }

    fn row_is_valid(&self, row: ProjectMenuRow) -> bool {
        match row {
            ProjectMenuRow::Project(index) => index < self.options.len(),
            ProjectMenuRow::Machine | ProjectMenuRow::NewProject => true,
        }
    }
}

/// A real GPUI trigger and deferred, bounded menu over [`ProjectMenuState`].
pub struct NativeProjectMenu {
    state: ProjectMenuState,
    theme: ArtisanTheme,
    trigger_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu_scroll: ScrollHandle,
    trigger_origin: Rc<RefCell<Option<Point<Pixels>>>>,
    initial_reveal_flat: Rc<RefCell<Option<usize>>>,
    suppress_trigger_release: bool,
}

impl NativeProjectMenu {
    /// Builds the menu over an initial catalog and current target.
    pub fn new(
        options: Vec<ProjectMenuOption>,
        current: Option<ProjectMenuCurrent>,
        mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: ProjectMenuState::new(options, current),
            theme: ArtisanTheme::for_mode(mode),
            trigger_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            menu_focus: cx.focus_handle(),
            menu_scroll: ScrollHandle::new(),
            trigger_origin: Rc::new(RefCell::new(None)),
            initial_reveal_flat: Rc::new(RefCell::new(None)),
            suppress_trigger_release: false,
        }
    }

    /// Read-only access to the interaction state.
    #[must_use]
    pub fn state(&self) -> &ProjectMenuState {
        &self.state
    }

    /// The trigger's focus handle: the tab stop while closed and the
    /// restoration target after dismissal.
    #[must_use]
    pub fn trigger_focus(&self) -> &FocusHandle {
        &self.trigger_focus
    }

    /// The open menu panel's focus handle.
    #[must_use]
    pub fn menu_focus(&self) -> &FocusHandle {
        &self.menu_focus
    }

    /// Repoints the surface onto a different current target.
    pub fn set_current(&mut self, current: Option<ProjectMenuCurrent>, cx: &mut Context<Self>) {
        self.state.set_current(current);
        cx.notify();
    }

    /// Sets whether the trigger refuses interaction.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.state.set_disabled(disabled);
        if disabled {
            self.initial_reveal_flat.borrow_mut().take();
        }
        cx.notify();
    }

    /// Consumes the one pending selection event.
    pub fn take_pending_event(&mut self) -> Option<ProjectMenuEvent> {
        self.state.take_pending_event()
    }

    fn handle_trigger_click(
        &mut self,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, ClickEvent::Keyboard(_)) && self.suppress_trigger_release {
            self.suppress_trigger_release = false;
            return;
        }
        self.suppress_trigger_release = false;
        self.state.press_trigger();
        self.sync_focus_after_transition(window, cx);
        cx.notify();
    }

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
        let handled = match keystroke.key.as_str() {
            "down" if !keystroke.modifiers.modified() => {
                self.state.move_next();
                true
            }
            "up" if !keystroke.modifiers.modified() => {
                self.state.move_previous();
                true
            }
            "home" if !keystroke.modifiers.modified() => {
                self.state.move_first();
                true
            }
            "end" if !keystroke.modifiers.modified() => {
                self.state.move_last();
                true
            }
            "enter" | "space" if !keystroke.modifiers.modified() => {
                self.state.activate_highlighted();
                self.sync_focus_after_transition(window, cx);
                self.suppress_trigger_release = true;
                true
            }
            "escape" => {
                self.state.dismiss();
                window.focus(&self.trigger_focus, cx);
                true
            }
            _ => false,
        };
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
        if self.menu_scroll.bounds().contains(&event.position) {
            return;
        }
        self.state.dismiss();
        window.focus(&self.trigger_focus, cx);
        cx.notify();
    }

    fn choose_row(&mut self, row: ProjectMenuRow, window: &mut Window, cx: &mut Context<Self>) {
        self.state.activate_row(row);
        self.sync_focus_after_transition(window, cx);
        cx.notify();
    }

    fn sync_focus_after_transition(&mut self, window: &mut Window, cx: &mut App) {
        if self.state.is_open() {
            window.focus(&self.menu_focus, cx);
            let flat = self
                .state
                .highlighted_row()
                .map_or(0, |row| self.state.flat_index(row));
            *self.initial_reveal_flat.borrow_mut() = Some(flat);
            self.reveal_highlight();
        } else {
            self.initial_reveal_flat.borrow_mut().take();
            window.focus(&self.trigger_focus, cx);
        }
    }

    fn reveal_highlight(&mut self) {
        if self.state.is_open()
            && let Some(row) = self.state.highlighted_row()
        {
            self.menu_scroll.scroll_to_item(self.state.flat_index(row));
        }
    }

    fn render_trigger(&self, cx: &Context<Self>) -> Stateful<Div> {
        let foreground = self.theme.colors.foreground.to_paint();
        let muted_foreground = self.theme.colors.muted_foreground.to_paint();
        let surface = self.theme.colors.muted.to_paint();
        let disabled = self.state.is_disabled();
        self.trigger_focus.clone().tab_stop(!disabled);

        div()
            .id("native-project-menu-trigger")
            .track_focus(&self.trigger_focus)
            .debug_selector(|| PROJECT_MENU_TRIGGER_SELECTOR.to_string())
            .on_key_down(cx.listener(Self::disarm_stale_release_fence))
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
            .child(
                div().size(px(8.0)).flex_shrink_0().rounded_full().bg(self
                    .theme
                    .colors
                    .primary
                    .to_paint()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(14.0))
                    .text_color(foreground)
                    .child(self.state.trigger_label()),
            )
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    AssetId::TABLER_SELECTOR,
                    IconSize::Default,
                    IconTint::Muted,
                ))
                .size(px(14.0))
                .flex_shrink_0()
                .text_color(muted_foreground),
            )
    }

    fn render_menu(&self, viewport: Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.state.is_open() {
            return None;
        }
        let trigger_origin = self.trigger_origin.borrow().as_ref().copied()?;
        let popover = self.theme.colors.popover.to_paint();
        let border = self.theme.colors.border.to_paint();
        let mut body = div()
            .id("native-project-menu")
            .track_focus(&self.menu_focus)
            .debug_selector(|| PROJECT_MENU_SELECTOR.to_string())
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

        for index in 0..self.state.options().len() {
            body = body.child(self.render_project_row(index, cx));
        }
        body = body.child(self.render_machine_row(cx));
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
                .anchor(Anchor::BottomLeft)
                .position(trigger_origin)
                .offset(point(px(0.0), px(-MENU_GAP_PX)))
                .child(body)
                .into_any_element(),
        )
    }

    fn render_project_row(&self, index: usize, cx: &Context<Self>) -> Stateful<Div> {
        let option = &self.state.options()[index];
        let selected = self.current_project_selected(&option.id);
        let style = ListRowStyle::resolve(
            self.theme,
            ListRowGeometry::Menu,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        );
        let content = match &option.path {
            Some(path) => ListRowContent::two_line(
                SharedString::from(option.name.clone()),
                SharedString::from(path.clone()),
            ),
            None => ListRowContent::one_line(SharedString::from(option.name.clone())),
        };
        let mut slots = ListRowSlots::new().leading(
            div().size(px(8.0)).flex_shrink_0().rounded_full().bg(self
                .theme
                .colors
                .primary
                .to_paint()),
        );
        if selected {
            slots = slots.trailing(
                div()
                    .text_size(px(16.0))
                    .line_height(style.title_line_height)
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child("✓"),
            );
        }
        self.selectable_row(
            list_row(style, content, slots),
            ProjectMenuRow::Project(index),
            index,
            cx,
        )
    }

    fn render_machine_row(&self, cx: &Context<Self>) -> Stateful<Div> {
        let selected = self.state.current() == Some(&ProjectMenuCurrent::Machine);
        let style = ListRowStyle::resolve(
            self.theme,
            ListRowGeometry::Menu,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        );
        let mut slots = ListRowSlots::new().leading(
            icon(IconStyle::resolve(
                self.theme,
                AssetId::TABLER_DEVICE_LAPTOP,
                IconSize::Default,
                IconTint::Muted,
            ))
            .size(px(16.0)),
        );
        if selected {
            slots = slots.trailing(
                div()
                    .text_size(px(16.0))
                    .line_height(style.title_line_height)
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child("✓"),
            );
        }
        let flat = self.state.flat_index(ProjectMenuRow::Machine);
        self.selectable_row(
            list_row(
                style,
                ListRowContent::one_line(SharedString::from(MACHINE_ROW_TITLE)),
                slots,
            ),
            ProjectMenuRow::Machine,
            flat,
            cx,
        )
    }

    fn render_new_project_row(&self, cx: &Context<Self>) -> Stateful<Div> {
        let style = ListRowStyle::resolve(
            self.theme,
            ListRowGeometry::Menu,
            ListRowTone::Foreground,
            FontWeight::NORMAL,
        );
        let slots = ListRowSlots::new().leading(
            icon(IconStyle::resolve(
                self.theme,
                AssetId::TABLER_FOLDER_PLUS,
                IconSize::Default,
                IconTint::Muted,
            ))
            .size(px(16.0)),
        );
        let flat = self.state.flat_index(ProjectMenuRow::NewProject);
        self.selectable_row(
            list_row(
                style,
                ListRowContent::one_line(SharedString::from(NEW_PROJECT_ROW_LABEL)),
                slots,
            ),
            ProjectMenuRow::NewProject,
            flat,
            cx,
        )
    }

    fn current_project_selected(&self, id: &str) -> bool {
        matches!(self.state.current(), Some(ProjectMenuCurrent::Project(current)) if current == id)
    }

    fn selectable_row(
        &self,
        presentation: Div,
        row: ProjectMenuRow,
        flat: usize,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let highlighted = self.state.highlighted_row() == Some(row);
        let selector = format!("{PROJECT_MENU_ROW_SELECTOR_PREFIX}-{flat}");
        presentation
            .id(SharedString::from(format!("project-menu-row-{flat}")))
            .on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.choose_row(row, window, cx);
                }),
            )
            .debug_selector(move || selector.clone())
            .when(highlighted, |entry| {
                entry.bg(self.theme.colors.accent.to_paint())
            })
    }
}

impl Render for NativeProjectMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        let menu = self.render_menu(viewport, cx);
        let trigger = self.render_trigger(cx);
        let probe_origin = Rc::clone(&self.trigger_origin);
        let probe_reveal = Rc::clone(&self.initial_reveal_flat);
        let probe_scroll = self.menu_scroll.clone();
        let probe = canvas(
            move |_, _, _| {},
            move |bounds, (), window, cx| {
                let moved = *probe_origin.borrow_mut() != Some(bounds.origin);
                *probe_origin.borrow_mut() = Some(bounds.origin);
                if let Some(flat) = probe_reveal.borrow_mut().take() {
                    let scroll = probe_scroll.clone();
                    window.defer(cx, move |window, _| {
                        scroll.scroll_to_item(flat);
                        window.refresh();
                    });
                } else if moved {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .size_full();

        div()
            .id("native-project-menu-root")
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

/// Menu panel width for a viewport: the legacy `min(20rem, 100vw - 2rem)`
/// clamp, floored at zero for degenerate windows.
#[must_use]
pub fn menu_width_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.width) - MENU_VIEWPORT_INSET_X_PX;
    px(MENU_WIDTH_PX.min(available.max(0.0)))
}

/// Bounded menu body height for a viewport: the leaf maximum, clamped so the
/// panel plus the reserved inset always fit inside the window.
#[must_use]
pub fn menu_max_height_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.height) - MENU_VIEWPORT_INSET_Y_PX;
    px(MENU_MAX_HEIGHT_PX.min(available.max(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_options() -> Vec<ProjectMenuOption> {
        vec![
            ProjectMenuOption::new("project-a", "Alpha").path("C:/work/alpha"),
            ProjectMenuOption::new("project-b", "Beta"),
        ]
    }

    #[test]
    fn trigger_label_names_current_target_or_invites() {
        let state = ProjectMenuState::new(fixture_options(), None);
        assert_eq!(state.trigger_label(), "Project: Choose a project");
        let state = ProjectMenuState::new(
            fixture_options(),
            Some(ProjectMenuCurrent::Project(String::from("project-b"))),
        );
        assert_eq!(state.trigger_label(), "Project: Beta");
        let state = ProjectMenuState::new(fixture_options(), Some(ProjectMenuCurrent::Machine));
        assert_eq!(state.trigger_label(), "Project: Machine: This computer");
    }

    #[test]
    fn open_highlights_current_target_first() {
        let mut state = ProjectMenuState::new(
            fixture_options(),
            Some(ProjectMenuCurrent::Project(String::from("project-b"))),
        );
        state.press_trigger();
        assert!(state.is_open());
        assert_eq!(state.highlighted_row(), Some(ProjectMenuRow::Project(1)));
        state.dismiss();
        let mut machine =
            ProjectMenuState::new(fixture_options(), Some(ProjectMenuCurrent::Machine));
        machine.press_trigger();
        assert_eq!(machine.highlighted_row(), Some(ProjectMenuRow::Machine));
        let mut empty = ProjectMenuState::new(Vec::new(), None);
        empty.press_trigger();
        assert_eq!(empty.highlighted_row(), Some(ProjectMenuRow::Machine));
    }

    #[test]
    fn keyboard_movement_wraps_across_machine_and_new_rows() {
        let mut state = ProjectMenuState::new(fixture_options(), None);
        state.press_trigger();
        assert_eq!(state.selectable_row_count(), 4);
        assert_eq!(state.highlighted_row(), Some(ProjectMenuRow::Project(0)));
        state.move_previous();
        assert_eq!(state.highlighted_row(), Some(ProjectMenuRow::NewProject));
        state.move_next();
        assert_eq!(state.highlighted_row(), Some(ProjectMenuRow::Project(0)));
        state.move_last();
        assert_eq!(state.highlighted_row(), Some(ProjectMenuRow::NewProject));
        state.move_first();
        assert_eq!(state.highlighted_row(), Some(ProjectMenuRow::Project(0)));
    }

    #[test]
    fn selection_closes_first_and_emits_exactly_once() {
        let mut state = ProjectMenuState::new(
            fixture_options(),
            Some(ProjectMenuCurrent::Project(String::from("project-a"))),
        );
        state.press_trigger();
        state.move_next();
        assert_eq!(state.highlighted_row(), Some(ProjectMenuRow::Project(1)));
        assert_eq!(state.pending_event(), None);
        state.activate_highlighted();
        assert!(!state.is_open());
        assert_eq!(
            state.pending_event(),
            Some(&ProjectMenuEvent::ChooseProject {
                project_id: String::from("project-b")
            })
        );
        state.activate_row(ProjectMenuRow::NewProject);
        assert_eq!(
            state.pending_event(),
            Some(&ProjectMenuEvent::ChooseProject {
                project_id: String::from("project-b")
            })
        );
        assert_eq!(
            state.take_pending_event(),
            Some(ProjectMenuEvent::ChooseProject {
                project_id: String::from("project-b")
            })
        );
        assert!(state.take_pending_event().is_none());
    }

    #[test]
    fn activating_current_target_or_machine_emits_nothing() {
        let mut state = ProjectMenuState::new(
            fixture_options(),
            Some(ProjectMenuCurrent::Project(String::from("project-a"))),
        );
        state.press_trigger();
        state.activate_highlighted();
        assert!(!state.is_open());
        assert!(state.pending_event().is_none());

        let mut machine =
            ProjectMenuState::new(fixture_options(), Some(ProjectMenuCurrent::Machine));
        machine.press_trigger();
        machine.activate_highlighted();
        assert!(machine.pending_event().is_none());

        machine.press_trigger();
        machine.activate_row(ProjectMenuRow::Machine);
        assert!(machine.pending_event().is_none());
        machine.press_trigger();
        machine.activate_row(ProjectMenuRow::NewProject);
        assert_eq!(
            machine.take_pending_event(),
            Some(ProjectMenuEvent::NewProject)
        );
    }

    #[test]
    fn machine_row_emits_typed_machine_event() {
        let mut state = ProjectMenuState::new(fixture_options(), None);
        state.press_trigger();
        state.activate_row(ProjectMenuRow::Machine);
        assert_eq!(
            state.take_pending_event(),
            Some(ProjectMenuEvent::ChooseMachine)
        );
    }

    #[test]
    fn disabled_refuses_open_and_discards_pending() {
        let mut state = ProjectMenuState::new(fixture_options(), None);
        state.press_trigger();
        state.activate_row(ProjectMenuRow::NewProject);
        assert!(state.pending_event().is_some());
        state.set_disabled(true);
        assert!(!state.is_open());
        assert!(state.pending_event().is_none());
        state.set_disabled(false);
        state.press_trigger();
        assert!(state.is_open());
    }
}
