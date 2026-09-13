//! GPUI render and interaction surface for the command menu.
//!
//! Extracted verbatim from `native_command_menu.rs` during the module split.

#[allow(clippy::wildcard_imports)]
use super::*;

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
            }
            "down" if !keystroke.modifiers.modified() => {
                self.state.move_next();
                self.reveal_highlight();
                cx.notify();
            }
            "up" if !keystroke.modifiers.modified() => {
                self.state.move_previous();
                self.reveal_highlight();
                cx.notify();
            }
            "home" if !keystroke.modifiers.modified() => {
                self.state.move_first();
                self.reveal_highlight();
                cx.notify();
            }
            "end" if !keystroke.modifiers.modified() => {
                self.state.move_last();
                self.reveal_highlight();
                cx.notify();
            }
            "enter" if !keystroke.modifiers.modified() => {
                if self.state.activate_highlighted().is_some() {
                    self.focus_return(window, cx);
                }
                cx.notify();
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
        entry.action.icon()
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
            .border_color(
                if self.input_focus.is_focused(window) && window.last_input_was_keyboard() {
                    self.desktop_theme.secondary
                } else {
                    self.desktop_theme.field_line
                },
            )
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
            .absolute()
            .top_0()
            .left_0()
            .size_full()
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
        self.input_bounds
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the offset is non-negative after max(0.0) and the rounded estimate is bounded by the query length the caller indexes with nth"
    )]
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

pub(super) fn utf8_offset_to_utf16(text: &str, offset: usize) -> Option<usize> {
    (offset <= text.len() && text.is_char_boundary(offset))
        .then(|| text[..offset].encode_utf16().count())
}

fn utf16_range_to_utf8(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    Some(utf16_offset_to_utf8(text, range.start)?..utf16_offset_to_utf8(text, range.end)?)
}

pub(super) fn utf16_offset_to_utf8(text: &str, offset: usize) -> Option<usize> {
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

pub(super) fn previous_character_boundary(text: &str, offset: usize) -> usize {
    if offset == 0 {
        return 0;
    }
    text[..offset]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

pub(super) fn next_character_boundary(text: &str, offset: usize) -> usize {
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
