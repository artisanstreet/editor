//! Home-surface inline project switcher: the opening affordance of the native
//! home route.
//!
//! This leaf pairs the proven [`ProjectPickerState`] interaction model
//! (controlled open state, current-row initial highlight, wrap-around arrow
//! movement, Home/End jumps, Enter/Space activation with close-before-action
//! emission, Escape/outside-press dismissal) with the home headline
//! presentation: an inline dotted-style project name inside the centered
//! heading, opening a floating dark rounded dropdown above the name with a
//! live "Search projects" filter row, folder-icon rows, a selected check,
//! scrolling, and the genuine "New project" intake row.
//!
//! Deliberate differences from [`ProjectPickerView`], all required by the
//! home design:
//!
//! - the trigger is an inline underlined name span composed into the heading,
//!   not a 280 px menu row;
//! - rows carry the existing Artisan folder glyph instead of the picker's
//!   identity dot;
//! - printable keystrokes feed a real case-insensitive substring filter over
//!   project names (Backspace shrinks it) rather than the picker's prefix
//!   typeahead; an empty filter is exactly the legacy full-catalog behavior.
//!   There is no caret or IME bridge (pinned-GPUI honesty, like the picker's
//!   documented platform limits): Space still activates, so filters cannot
//!   contain spaces;
//! - there is no projectless row: the wired application has no genuine
//!   deselect operation, and this leaf emits no fake one.
//!
//! Placement and traversal reuse the audited [`ProjectPickerView`] pattern
//! (`deferred` + window-mode `anchored()` against a probe-recorded trigger
//! origin, height-bounded scroll body with one direct child per selectable
//! row, native tab-stop trigger with focus restore, release-fence against
//! the synthesized keyboard click).

#![allow(clippy::module_name_repetitions)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use artisan_assets::AssetId;
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::DesktopTheme;
use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, Div, FocusHandle, HighlightStyle,
    InteractiveElement as _, KeyDownEvent, MouseDownEvent, ParentElement as _, Pixels, Point,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, StyledText,
    UnderlineStyle, Window, anchored, canvas, deferred, div, point, prelude::FluentBuilder as _,
    prelude::IntoElement, px,
};

use crate::project_picker::{PickerRow, ProjectOption, ProjectPickerAction, ProjectPickerState};

/// Accessible invitation label for the trigger with no project selected.
pub const HOME_CHOOSE_PROJECT_LABEL: &str = "Choose a project";
/// Visible filter placeholder painted in the dropdown's fixed filter row.
pub const HOME_FILTER_PLACEHOLDER: &str = "Search projects";
/// Label of the distinct final intake row, matching the picker leaf.
const HOME_NEW_PROJECT_LABEL: &str = "New project";
/// Preferred painted width of the open menu panel, matching the picker leaf.
const HOME_MENU_WIDTH_PX: f32 = 320.0;
/// Bounded maximum menu body height; the panel scrolls past this.
const HOME_MENU_MAX_HEIGHT_PX: f32 = 360.0;
/// Horizontal viewport inset reserved by the width clamp.
const HOME_MENU_VIEWPORT_INSET_X_PX: f32 = 32.0;
/// Vertical viewport inset reserved when bounding the menu height.
const HOME_MENU_VIEWPORT_INSET_Y_PX: f32 = 32.0;
/// Gap between the trigger and the panel above it (legacy `sideOffset={10}`).
const HOME_MENU_GAP_PX: f32 = 10.0;
/// Debug selector painted on the inline trigger span.
pub const HOME_TRIGGER_SELECTOR: &str = "artisan-home-project-trigger";
/// Debug selector painted on the open menu panel.
pub const HOME_MENU_SELECTOR: &str = "artisan-home-project-menu";
/// Debug selector painted on the filter row.
pub const HOME_FILTER_SELECTOR: &str = "artisan-home-project-filter";
/// Headline text size shared by the home heading and the inline trigger.
pub const HOME_HEADLINE_TEXT_PX: f32 = 28.0;
/// Muted emblem size above the home heading.
pub const HOME_EMBLEM_SIZE_PX: f32 = 30.0;
/// Prefix of the debug selectors painted on selectable rows.
const HOME_ROW_SELECTOR_PREFIX: &str = "artisan-home-project-row";

/// Menu panel width for a viewport: the legacy `min(20rem, 100vw - 2rem)`
/// clamp, floored at zero for degenerate windows.
fn menu_width_for_viewport(viewport: gpui::Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.width) - HOME_MENU_VIEWPORT_INSET_X_PX;
    px(HOME_MENU_WIDTH_PX.min(available.max(0.0)))
}

/// Bounded menu body height for a viewport: the leaf maximum, clamped so the
/// panel plus the reserved inset always fit inside the window.
fn menu_max_height_for_viewport(viewport: gpui::Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.height) - HOME_MENU_VIEWPORT_INSET_Y_PX;
    px(HOME_MENU_MAX_HEIGHT_PX.min(available.max(0.0)))
}

/// Native GPUI presentation of the home inline switcher.
///
/// Owns a [`ProjectPickerState`], drains its actions, applies `Choose` back
/// as the controller repointing, and keeps the last applied action readable
/// for the application router and tests.
pub struct HomeProjectPickerView {
    state: ProjectPickerState,
    theme: DesktopTheme,
    trigger_focus: FocusHandle,
    menu_focus: FocusHandle,
    /// Scroll state of the open menu body; one direct child per selectable
    /// row keeps `scroll_to_item` indexes aligned with visible flat
    /// addresses.
    menu_scroll: gpui::ScrollHandle,
    /// Painted window-space origin of the inline trigger, refreshed every
    /// frame by an invisible probe element.
    trigger_origin: Rc<RefCell<Option<Point<Pixels>>>>,
    /// Flat visible-row address waiting to be revealed once the fresh scroll
    /// handle has received real bounds (same pinned quirk as the picker).
    initial_reveal_flat: Rc<Cell<Option<usize>>>,
    /// Release fence against the synthesized keyboard click, mirroring the
    /// picker leaf.
    suppress_trigger_release: bool,
    /// Last drained action, readable for the application router.
    last_action: Option<ProjectPickerAction>,
}

impl HomeProjectPickerView {
    /// Builds the view over an initial catalog and current project.
    pub fn new(
        projects: Vec<ProjectOption>,
        current: Option<artisan_domain::ProjectId>,
        theme: DesktopTheme,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: ProjectPickerState::new(projects, current),
            theme,
            trigger_focus: cx.focus_handle().tab_index(0).tab_stop(true),
            menu_focus: cx.focus_handle(),
            menu_scroll: gpui::ScrollHandle::new(),
            trigger_origin: Rc::new(RefCell::new(None)),
            initial_reveal_flat: Rc::new(Cell::new(None)),
            suppress_trigger_release: false,
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

    /// Sets whether the trigger refuses interaction.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.state.set_disabled(disabled);
        if disabled {
            self.initial_reveal_flat.set(None);
        }
        cx.notify();
    }

    /// Toggles the controlled open state without touching focus.
    ///
    /// Window-free controller seam (mirrors the picker's trigger path):
    /// real pointer/keyboard callers use [`Self::press_inline_trigger`],
    /// which adds focus sync; tests drive this seam directly.
    pub fn toggle_menu(&mut self, cx: &mut Context<Self>) {
        self.state.press_trigger();
        if self.state.is_open() {
            let flat = self.state.flat_index_of(
                self.state
                    .highlighted_row()
                    .unwrap_or(PickerRow::NewProject),
            );
            self.initial_reveal_flat.set(Some(flat));
            self.reveal_highlight();
        } else {
            self.initial_reveal_flat.set(None);
        }
        cx.notify();
    }

    /// Activates one row without touching focus.
    ///
    /// Window-free controller seam: closes first, then queues the action for
    /// the application router. Real callers use [`Self::choose_home_row`],
    /// which adds trigger-focus restore; tests drive this seam directly.
    pub fn commit_row(&mut self, row: PickerRow, cx: &mut Context<Self>) {
        self.state.activate_row(row);
        self.drain_actions();
        cx.notify();
    }

    /// Presses the inline trigger: toggles the menu and settles focus.
    pub fn press_inline_trigger(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_menu(cx);
        self.sync_focus_after_transition(window, cx);
    }

    /// Activates one row, then restores trigger focus.
    pub fn choose_home_row(
        &mut self,
        row: PickerRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_row(row, cx);
        self.sync_focus_after_transition(window, cx);
    }

    /// Renders the inline trigger span for composition into the home
    /// heading: the underlined project name (or `label` with no selection)
    /// plus the origin probe and the deferred open menu.
    pub fn render_inline_trigger(
        &self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let menu = self.render_menu(viewport, cx);
        let disabled = self.state.is_disabled();
        self.trigger_focus.clone().tab_stop(!disabled);

        let name = SharedString::from(label.to_owned());
        let underlined = StyledText::new(name.clone()).with_highlights([(
            0..name.len(),
            HighlightStyle {
                underline: Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(self.theme.secondary),
                    wavy: false,
                }),
                ..Default::default()
            },
        )]);

        let probe_origin = Rc::clone(&self.trigger_origin);
        let probe_reveal = Rc::clone(&self.initial_reveal_flat);
        let probe_scroll = self.menu_scroll.clone();
        let probe = canvas(
            move |_, _, _| {},
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

        div()
            .id("home-project-trigger-root")
            .tab_group()
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
            .debug_selector(|| HOME_TRIGGER_SELECTOR.to_string())
            .relative()
            .text_size(px(HOME_HEADLINE_TEXT_PX))
            .text_color(self.theme.foreground)
            .child(probe)
            .children(menu.map(deferred))
            .child(
                div()
                    .id("home-project-trigger")
                    .track_focus(&self.trigger_focus)
                    .on_key_down(cx.listener(Self::disarm_stale_release_fence))
                    .when(!disabled, |span| {
                        span.on_click(cx.listener(Self::handle_trigger_click))
                    })
                    .when(disabled, |span| span.opacity(0.5))
                    .child(underlined),
            )
            .into_any_element()
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
        self.press_inline_trigger(window, cx);
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

    fn handle_home_key(
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
                self.commit_row_from_highlight(window, cx);
                self.suppress_trigger_release = true;
                true
            }
            "escape" => {
                self.dismiss_and_settle(window, cx);
                true
            }
            "backspace" => {
                let mut filter = self.state.filter().to_owned();
                filter.pop();
                self.state.set_filter(filter);
                true
            }
            _ => false,
        };

        // Printable keystrokes feed the live substring filter (the picker's
        // prefix typeahead stays dormant on this surface); Space keeps its
        // activation meaning above, so filters cannot contain spaces.
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
            let mut filter = self.state.filter().to_owned();
            filter.push(typed);
            self.state.set_filter(filter);
            handled = true;
        }

        if handled {
            self.reveal_highlight();
            cx.notify();
        }
    }

    fn commit_row_from_highlight(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(row) = self.state.highlighted_row() {
            self.choose_home_row(row, window, cx);
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
        self.dismiss_and_settle(window, cx);
        cx.notify();
    }

    fn dismiss_and_settle(&mut self, window: &mut Window, cx: &mut App) {
        self.state.dismiss();
        window.focus(&self.trigger_focus, cx);
    }

    fn sync_focus_after_transition(&mut self, window: &mut Window, cx: &mut App) {
        if self.state.is_open() {
            window.focus(&self.menu_focus, cx);
            let flat = self.state.flat_index_of(
                self.state
                    .highlighted_row()
                    .unwrap_or(PickerRow::NewProject),
            );
            self.initial_reveal_flat.set(Some(flat));
            self.reveal_highlight();
        } else {
            self.initial_reveal_flat.set(None);
            window.focus(&self.trigger_focus, cx);
        }
    }

    fn reveal_highlight(&mut self) {
        if !self.state.is_open() {
            return;
        }
        let flat = self.state.flat_index_of(
            self.state
                .highlighted_row()
                .unwrap_or(PickerRow::NewProject),
        );
        self.menu_scroll.scroll_to_item(flat);
    }

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

    /// Renders the open menu panel: fixed filter row above a height-bounded
    /// scroll body with one direct child per selectable row, anchored above
    /// the trigger. Frames before the probe records an origin render
    /// nothing.
    fn render_menu(&self, viewport: gpui::Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.state.is_open() {
            return None;
        }
        let trigger_origin = self.trigger_origin.borrow().as_ref().copied()?;

        let mut body = div()
            .id("home-project-menu")
            .track_focus(&self.menu_focus)
            .debug_selector(|| HOME_MENU_SELECTOR.to_string())
            .on_key_down(cx.listener(Self::handle_home_key))
            .flex()
            .flex_col()
            .w(menu_width_for_viewport(viewport))
            .rounded(px(16.0))
            .bg(self.theme.sidebar)
            .border_1()
            .border_color(self.theme.popover_line)
            .child(self.render_filter_row());

        let mut list = div()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .max_h(menu_max_height_for_viewport(viewport))
            .p(px(4.0));
        for catalog_index in self.state.visible_indexes() {
            list = list.child(self.render_project_row(catalog_index, cx));
        }
        list = list.child(
            div()
                .flex()
                .flex_col()
                .child(
                    separator(self.theme.popover_line, SeparatorAxis::Horizontal)
                        .my(px(4.0))
                        .mx(px(4.0)),
                )
                .child(self.render_new_project_row(cx)),
        );
        body = body.child(list);

        Some(
            anchored()
                .anchor(Anchor::BottomLeft)
                .position(trigger_origin)
                .offset(point(px(0.0), px(-HOME_MENU_GAP_PX)))
                .child(body)
                .into_any_element(),
        )
    }

    /// Fixed filter row: the live filter text or the muted invitation copy.
    fn render_filter_row(&self) -> Div {
        let filter = self.state.filter();
        let (text, muted) = if filter.is_empty() {
            (HOME_FILTER_PLACEHOLDER, true)
        } else {
            (filter, false)
        };
        div()
            .debug_selector(|| HOME_FILTER_SELECTOR.to_string())
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(8.0))
            .child(
                asset_glyph(AssetId::TABLER_SEARCH)
                    .size(px(15.0))
                    .text_color(self.theme.secondary),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(13.0))
                    .text_color(if muted {
                        self.theme.secondary
                    } else {
                        self.theme.foreground
                    })
                    .child(text.to_owned()),
            )
    }

    /// One visible project row: folder glyph, one-line name, selected check.
    fn render_project_row(&self, catalog_index: usize, cx: &Context<Self>) -> Stateful<Div> {
        let option = &self.state.projects()[catalog_index];
        let selected = self
            .state
            .current_id()
            .is_some_and(|current| current == &option.id);
        let highlighted = self.state.highlighted_row() == Some(PickerRow::Project(catalog_index));
        let row_selector = format!("{HOME_ROW_SELECTOR_PREFIX}-{catalog_index}");
        div()
            .id(SharedString::from(format!("home-project-row-{catalog_index}")))
            .debug_selector(move || row_selector.clone())
            .on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, window, context| {
                view.choose_home_row(PickerRow::Project(catalog_index), window, context);
            }))
            .flex()
            .items_center()
            .w_full()
            .gap(px(10.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(12.0))
            .when(highlighted, |entry| entry.bg(self.theme.selected))
            .child(
                asset_glyph(AssetId::TABLER_FOLDER)
                    .size(px(16.0))
                    .text_color(self.theme.secondary),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(14.0))
                    .text_color(self.theme.foreground)
                    .child(option.name.clone()),
            )
            .when(selected, |entry| {
                entry.child(
                    div()
                        .text_size(px(16.0))
                        .text_color(self.theme.secondary)
                        .child("✓"),
                )
            })
    }

    /// The distinct final intake row after the hairline separator.
    fn render_new_project_row(&self, cx: &Context<Self>) -> Stateful<Div> {
        div()
            .id("home-project-row-new")
            .debug_selector(|| format!("{HOME_ROW_SELECTOR_PREFIX}-new"))
            .on_click(cx.listener(|view: &mut Self, _: &ClickEvent, window, context| {
                view.choose_home_row(PickerRow::NewProject, window, context);
            }))
            .flex()
            .items_center()
            .w_full()
            .gap(px(10.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(12.0))
            .when(
                self.state.highlighted_row() == Some(PickerRow::NewProject),
                |entry| entry.bg(self.theme.selected),
            )
            .child(
                div()
                    .w(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .flex_shrink_0()
                    .text_size(px(16.0))
                    .text_color(self.theme.secondary)
                    .child("+"),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(14.0))
                    .text_color(self.theme.foreground)
                    .child(HOME_NEW_PROJECT_LABEL),
            )
    }
}
