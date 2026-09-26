//! Project picker behind the home headline's inline trigger: the new-task
//! surface's choice of project.
//!
//! The trigger uses the controlled selection, focus, and typeahead policy
//! from `ProjectPickerState`. Menus use the same glass material as the turn
//! navigator. Project rows stay visible while typing jumps the highlight;
//! Enter or Space selects, and Escape or an outside press dismisses.

#![allow(clippy::module_name_repetitions)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use crate::native_composer_material::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use artisan_assets::AssetId;
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::{ArtisanTheme, DesktopTheme, ThemeMode};
use gpui::{
    Anchor, AnyElement, App, Bounds, ClickEvent, Context, Div, FocusHandle, HighlightStyle,
    InteractiveElement as _, KeyDownEvent, MouseDownEvent, ParentElement as _, Pixels,
    SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, StyledText,
    UnderlineStyle, Window, anchored, canvas, deferred, div, point, prelude::FluentBuilder as _,
    prelude::IntoElement, px,
};

use crate::native_model_selector::{HoverRect, SlidingHoverState, render_picker_hover_pill};
use crate::project_picker::{PickerRow, ProjectOption, ProjectPickerAction, ProjectPickerState};

/// Accessible invitation label for the trigger with no project selected.
pub const HOME_CHOOSE_PROJECT_LABEL: &str = "Choose a project";
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
/// Shared radius for the menu's project rows.
const PROJECT_CONTROL_RADIUS_PX: f32 = 6.0;
const PROJECT_MENU_RADIUS_PX: f32 = 10.0;
/// Debug selector painted on the inline trigger span.
pub const HOME_TRIGGER_SELECTOR: &str = "artisan-home-project-trigger";
/// Debug selector painted on the open menu panel.
pub const HOME_MENU_SELECTOR: &str = "artisan-home-project-menu";
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
    /// Painted window-space bounds of the trigger, refreshed every
    /// frame by an invisible probe element.
    trigger_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    /// Flat visible-row address waiting to be revealed once the fresh scroll
    /// handle has received real bounds (same pinned quirk as the picker).
    initial_reveal_flat: Rc<Cell<Option<usize>>>,
    /// Release fence against the synthesized keyboard click, mirroring the
    /// picker leaf.
    suppress_trigger_release: bool,
    /// Last drained action, readable for the application router.
    last_action: Option<ProjectPickerAction>,
    typeahead_clock: Instant,
    menu_hover: Rc<RefCell<SlidingHoverState>>,
    menu_hover_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    menu_hover_keyboard: bool,
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
            trigger_bounds: Rc::new(RefCell::new(None)),
            initial_reveal_flat: Rc::new(Cell::new(None)),
            suppress_trigger_release: false,
            last_action: None,
            typeahead_clock: Instant::now(),
            menu_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            menu_hover_bounds: Rc::new(RefCell::new(None)),
            menu_hover_keyboard: false,
        }
    }

    /// Read-only access to the underlying interaction state.
    #[must_use]
    pub fn state(&self) -> &ProjectPickerState {
        &self.state
    }

    #[cfg(test)]
    pub(crate) fn menu_hover_state(&self) -> std::cell::Ref<'_, SlidingHoverState> {
        self.menu_hover.borrow()
    }

    fn menu_hover_id(&self, row: PickerRow) -> String {
        match row {
            PickerRow::Project(index) => {
                format!("project:{}", self.state.projects()[index].id.as_str())
            }
            PickerRow::NewProject => "new-project".to_owned(),
        }
    }

    fn clear_menu_hover(&mut self) {
        self.menu_hover.borrow_mut().clear();
        self.menu_hover_keyboard = false;
    }

    fn sync_menu_hover_from_keyboard(&mut self) {
        if let Some(row) = self.state.highlighted_row() {
            self.menu_hover_keyboard = true;
            self.menu_hover
                .borrow_mut()
                .set_active(self.menu_hover_id(row));
        } else {
            self.clear_menu_hover();
        }
    }

    fn hover_menu_row(&mut self, row: PickerRow, cx: &mut Context<Self>) {
        if !self.state.is_open() {
            return;
        }
        let id = self.menu_hover_id(row);
        if !self.menu_hover_keyboard && self.menu_hover.borrow().active_id() == Some(id.as_str()) {
            return;
        }
        self.menu_hover_keyboard = false;
        self.state.highlight_row(row);
        self.menu_hover.borrow_mut().set_active(id);
        cx.notify();
    }

    // The panel is the coordinate origin so scrolling rows remeasure against
    // its viewport, while the scroll list keeps one child per selectable row.
    fn menu_hover_probe(&self, row: Option<PickerRow>) -> AnyElement {
        let id = row.map(|row| self.menu_hover_id(row));
        let hover = Rc::clone(&self.menu_hover);
        let panel = Rc::clone(&self.menu_hover_bounds);
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let changed = if let Some(id) = &id {
                    let Some(panel) = *panel.borrow() else {
                        return;
                    };
                    hover.borrow_mut().measure(
                        id,
                        HoverRect {
                            left: f32::from(bounds.left() - panel.left()),
                            top: f32::from(bounds.top() - panel.top()),
                            width: f32::from(bounds.size.width),
                            height: f32::from(bounds.size.height),
                        },
                    )
                } else {
                    let mut panel = panel.borrow_mut();
                    let changed = *panel != Some(bounds);
                    *panel = Some(bounds);
                    changed
                };
                if changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .into_any_element()
    }

    /// The last action this view drained and applied, if any.
    #[must_use]
    pub fn last_action(&self) -> Option<ProjectPickerAction> {
        self.last_action.clone()
    }

    /// Updates the displayed selection when the application changes projects.
    pub fn set_current(
        &mut self,
        current: Option<artisan_domain::ProjectId>,
        cx: &mut Context<Self>,
    ) {
        if self.state.current_id() != current.as_ref() {
            self.state.set_current(current);
            self.last_action = None;
            cx.notify();
        }
    }

    /// Sets whether the trigger refuses interaction.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        self.state.set_disabled(disabled);
        if disabled {
            self.clear_menu_hover();
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
        self.sync_menu_hover_from_keyboard();
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
        self.clear_menu_hover();
        self.drain_actions();
        cx.notify();
    }

    /// Presses the inline trigger: toggles the menu and settles focus.
    pub fn press_inline_trigger(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_menu(cx);
        self.sync_focus_after_transition(window, cx);
    }

    /// Activates one row, then restores trigger focus.
    pub fn choose_home_row(&mut self, row: PickerRow, window: &mut Window, cx: &mut Context<Self>) {
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

        div()
            .id("home-project-trigger-root")
            .tab_group()
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
            .debug_selector(|| HOME_TRIGGER_SELECTOR.to_string())
            .relative()
            .text_size(px(HOME_HEADLINE_TEXT_PX))
            .text_color(self.theme.foreground)
            .child(self.render_trigger_probe())
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

    fn render_trigger_probe(&self) -> AnyElement {
        let probe_bounds = Rc::clone(&self.trigger_bounds);
        let probe_reveal = Rc::clone(&self.initial_reveal_flat);
        let probe_scroll = self.menu_scroll.clone();
        canvas(
            move |_, _, _| {},
            move |bounds, (), window, cx| {
                let moved = *probe_bounds.borrow() != Some(bounds);
                *probe_bounds.borrow_mut() = Some(bounds);
                if let Some(flat) = probe_reveal.take() {
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
        .size_full()
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
            _ => false,
        };

        // Typeahead moves the highlight without hiding any projects.
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
            let now_ms =
                u64::try_from(self.typeahead_clock.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.state.handle_typeahead(typed, now_ms);
            handled = true;
        }

        if handled {
            cx.stop_propagation();
            self.sync_menu_hover_from_keyboard();
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
        self.clear_menu_hover();
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

    /// Renders the glass menu above the trigger with one direct child per
    /// selectable row. Frames before the probe records the trigger bounds
    /// render nothing.
    fn render_menu(&self, viewport: gpui::Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.state.is_open() {
            return None;
        }
        let trigger_bounds = self.trigger_bounds.borrow().as_ref().copied()?;
        let (anchor, position, gap) =
            (Anchor::BottomLeft, trigger_bounds.origin, -HOME_MENU_GAP_PX);

        let material_theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let mut body = div()
            .id("home-project-menu")
            .track_focus(&self.menu_focus)
            .debug_selector(|| HOME_MENU_SELECTOR.to_string())
            .on_key_down(cx.listener(Self::handle_home_key))
            .flex()
            .flex_col()
            .w(menu_width_for_viewport(viewport))
            .relative()
            .rounded(px(PROJECT_MENU_RADIUS_PX))
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(&material_theme))
            .shadow(glass_card_shadows())
            .child(glass_material_layer(
                GlassStrength::Strong,
                px(PROJECT_MENU_RADIUS_PX),
            ))
            .child(glass_highlight_layer(
                GlassStrength::Strong,
                px(PROJECT_MENU_RADIUS_PX),
            ));

        let mut list = div()
            .id("home-project-menu-list")
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .max_h(menu_max_height_for_viewport(viewport))
            .p(px(8.0));
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
        body = body
            .overflow_hidden()
            .on_hover(cx.listener(|view, hovered: &bool, _, cx| {
                if !*hovered && !view.menu_hover_keyboard {
                    view.menu_hover.borrow_mut().hide();
                    cx.notify();
                }
            }))
            .child(self.menu_hover_probe(None))
            .child(render_picker_hover_pill(
                &material_theme,
                &self.menu_hover,
                "project-menu",
                px(PROJECT_CONTROL_RADIUS_PX),
                cx.reduce_motion() || self.menu_hover_keyboard,
            ))
            .child(list);

        Some(
            anchored()
                .anchor(anchor)
                .position(position)
                .offset(point(px(0.0), px(gap)))
                .child(body)
                .into_any_element(),
        )
    }

    /// One visible project row: folder glyph, one-line name, selected check.
    fn render_project_row(&self, catalog_index: usize, cx: &Context<Self>) -> Stateful<Div> {
        let option = &self.state.projects()[catalog_index];
        let selected = self
            .state
            .current_id()
            .is_some_and(|current| current == &option.id);
        let row_selector = format!("{HOME_ROW_SELECTOR_PREFIX}-{catalog_index}");
        div()
            .id(SharedString::from(format!(
                "home-project-row-{catalog_index}"
            )))
            .debug_selector(move || row_selector.clone())
            .on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, context| {
                    view.choose_home_row(PickerRow::Project(catalog_index), window, context);
                }),
            )
            .flex()
            .items_center()
            .w_full()
            .gap(px(10.0))
            .px(px(8.0))
            .py(px(6.0))
            .relative()
            .rounded(px(PROJECT_CONTROL_RADIUS_PX))
            .cursor_pointer()
            .on_hover(cx.listener(move |view, hovered: &bool, _, cx| {
                if *hovered {
                    view.hover_menu_row(PickerRow::Project(catalog_index), cx);
                }
            }))
            .on_mouse_move(cx.listener(move |view, _, _, cx| {
                view.hover_menu_row(PickerRow::Project(catalog_index), cx);
            }))
            .child(self.menu_hover_probe(Some(PickerRow::Project(catalog_index))))
            .child(
                asset_glyph(AssetId::TABLER_FOLDER)
                    .size(px(16.0))
                    .text_color(self.theme.secondary),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
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
            .on_click(
                cx.listener(|view: &mut Self, _: &ClickEvent, window, context| {
                    view.choose_home_row(PickerRow::NewProject, window, context);
                }),
            )
            .flex()
            .items_center()
            .w_full()
            .gap(px(10.0))
            .px(px(8.0))
            .py(px(6.0))
            .relative()
            .rounded(px(PROJECT_CONTROL_RADIUS_PX))
            .cursor_pointer()
            .on_hover(cx.listener(|view, hovered: &bool, _, cx| {
                if *hovered {
                    view.hover_menu_row(PickerRow::NewProject, cx);
                }
            }))
            .on_mouse_move(cx.listener(|view, _, _, cx| {
                view.hover_menu_row(PickerRow::NewProject, cx);
            }))
            .child(self.menu_hover_probe(Some(PickerRow::NewProject)))
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
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(14.0))
                    .text_color(self.theme.foreground)
                    .child(HOME_NEW_PROJECT_LABEL),
            )
    }
}
