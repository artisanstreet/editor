//! The machine dropdown selects a Forge connection inside the existing editor window.
use super::*;
use std::path::PathBuf;

pub(super) struct SelectMachine(pub Option<PathBuf>);
impl gpui::EventEmitter<SelectMachine> for NativeApplication {}

pub(super) struct MachineMenu {
    pub(super) open: bool,
    #[cfg(not(test))]
    refreshing: bool,
    trigger_focus: FocusHandle,
    pub(super) bounds: Rc<Cell<Bounds<gpui::Pixels>>>,
    pub(super) focus: FocusHandle,
    pub(super) entries: Vec<CommandMenuEntry>,
    pub(super) highlighted: usize,
    pub(super) scroll: ScrollHandle,
    pub(super) hover: Rc<RefCell<SlidingHoverState>>,
    pub(super) hover_surface: Rc<RefCell<Option<Bounds<gpui::Pixels>>>>,
    pub(super) details: HashMap<String, crate::native_hosts::HostPresentation>,
}

impl MachineMenu {
    pub(super) fn is_open(&self) -> bool {
        self.open
    }
    pub(super) fn new(cx: &mut Context<NativeApplication>) -> Self {
        Self {
            open: false,
            #[cfg(not(test))]
            refreshing: false,
            trigger_focus: cx.focus_handle(),
            bounds: Rc::new(Cell::new(Bounds::default())),
            focus: cx.focus_handle(),
            entries: Vec::new(),
            highlighted: 0,
            scroll: ScrollHandle::new(),
            hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            hover_surface: Rc::new(RefCell::new(None)),
            details: HashMap::new(),
        }
    }
}

impl NativeApplication {
    pub(super) fn profile_display_name(&self, cx: &App) -> String {
        cx.try_global::<crate::native_account_identity::ArtisanAccountIdentity>()
            .map(|identity| identity.display_name.trim())
            .filter(|name| !name.is_empty())
            .map_or_else(
                || {
                    self.profile_name
                        .as_ref()
                        .or(self.profile_hostname.as_ref())
                        .map_or_else(|| "This computer".into(), |name| capitalize_label(name))
                },
                str::to_owned,
            )
    }

    pub(super) fn add_host(cx: &mut Context<Self>) {
        let selected = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Select a trusted Forge host invitation".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = selected.await else { return; };
            let Some(path) = paths.into_iter().next() else { return; };
            let result = cx.background_executor().spawn(async move {
                crate::native_hosts::import(&path)
            }).await;
            let _ = this.update(cx, |app, cx| match result {
                Ok(home) => {
                    app.sync_command_menu_groups(cx);
                    cx.emit(SelectMachine(Some(home)));
                }
                Err(error) => {
                    eprintln!("Host import failed: {error}");
                    app.machine_error = Some("Could not add this host. Select a valid Forge invitation from a trusted machine.".into());
                    cx.notify();
                }
            });
        }).detach();
    }

    pub(super) fn open_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.profile_menu.is_open() {
            self.profile_menu.set_open(true);
            self.begin_profile_menu_open(cx);
            self.ensure_profile_usage(false, None, cx);
        }
        self.update_machine_entries();
        #[cfg(not(test))]
        self.refresh_machines(cx);
        self.profile_hover
            .borrow_mut()
            .set_active("profile-host".to_owned());
        self.profile_hover_keyboard
            .set(window.last_input_was_keyboard());
        self.machine_menu.hover.borrow_mut().clear();
        if window.last_input_was_keyboard() {
            self.activate_machine_hover();
        }
        self.machine_menu.open = true;
        self.machine_menu.focus.focus(window, cx);
        cx.notify();
    }

    fn update_machine_entries(&mut self) {
        self.machine_menu.entries = crate::native_hosts::group().entries;
        self.machine_menu.details = self
            .machine_menu
            .entries
            .iter()
            .filter_map(|entry| {
                if let CommandMenuAction::OpenHost { home } = &entry.action {
                    Some((
                        entry.id.clone(),
                        crate::native_hosts::presentation(home.as_deref()),
                    ))
                } else {
                    None
                }
            })
            .collect();
        self.machine_menu.highlighted = self
            .machine_menu
            .entries
            .iter()
            .position(|entry| {
                matches!(&entry.action, CommandMenuAction::OpenHost { home }
                if crate::native_hosts::same_host(home.as_deref(), self.machine_home.as_deref()))
            })
            .unwrap_or(0);
    }

    #[cfg(not(test))]
    pub(super) fn refresh_machines(&mut self, cx: &mut Context<Self>) {
        if self.machine_menu.refreshing {
            return;
        }
        self.machine_menu.refreshing = true;
        let home = self.machine_home.clone();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .spawn(async move {
                    crate::native_hosts::refresh(home.as_deref());
                })
                .await;
            let _ = this.update(cx, |app, cx| {
                app.machine_menu.refreshing = false;
                // Preserve the pointed-to entry when discovery finishes while the menu is open.
                let highlighted = app
                    .machine_menu
                    .entries
                    .get(app.machine_menu.highlighted)
                    .map(|entry| entry.id.clone());
                app.update_machine_entries();
                if let Some(index) = highlighted.and_then(|id| {
                    app.machine_menu
                        .entries
                        .iter()
                        .position(|entry| entry.id == id)
                }) {
                    app.machine_menu.highlighted = index;
                }
                app.machine_label = crate::native_hosts::label(app.machine_home.as_deref());
                app.sync_command_menu_groups(cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn dismiss_machine_submenu(&mut self) {
        self.machine_menu.open = false;
    }

    pub(super) fn focus_machine_trigger(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_hover_keyboard.set(true);
        self.profile_hover
            .borrow_mut()
            .set_active("profile-host".to_owned());
        self.machine_menu.trigger_focus.focus(window, cx);
    }

    pub(super) fn close_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.machine_menu.open = false;
        self.machine_menu.trigger_focus.focus(window, cx);
        cx.notify();
    }

    pub(super) fn choose_machine(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = self
            .machine_menu
            .entries
            .get(index)
            .map(|entry| entry.action.clone())
        else {
            return;
        };
        self.close_machines(window, cx);
        match action {
            CommandMenuAction::OpenHost { home } => cx.emit(SelectMachine(home)),
            CommandMenuAction::AddHost => Self::add_host(cx),
            _ => {}
        }
    }

    pub(super) fn machine_trigger(
        &self,
        identity: AnyElement,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let bounds = Rc::clone(&self.machine_menu.bounds);
        let hover = Rc::clone(&self.profile_hover);
        let surface = Rc::clone(&self.profile_hover_surface_bounds);
        div()
            .id("machine-selector")
            .debug_selector(|| "machine-selector".into())
            .role(gpui::Role::Button)
            .aria_label("Select Forge host")
            .track_focus(&self.machine_menu.trigger_focus)
            .tab_index(0)
            .block_mouse_except_scroll()
            .cursor_pointer()
            .relative()
            .w_full()
            .px(px(12.0))
            .py(px(12.0))
            .rounded(px(10.0))
            .flex_shrink_0()
            .text_size(px(12.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .on_hover(cx.listener(|app, hovered: &bool, window, cx| {
                if *hovered {
                    app.profile_hover
                        .borrow_mut()
                        .set_active("profile-host".to_owned());
                    app.profile_hover_keyboard.set(false);
                    if !app.machine_menu.is_open() {
                        app.open_machines(window, cx);
                    }
                    cx.notify();
                }
            }))
            .child(identity)
            .child(desktop_nav_glyph(
                AssetId::TABLER_CHEVRON_RIGHT,
                self.desktop_theme,
            ))
            .child(
                canvas(
                    |_, _, _| {},
                    move |area, (), window, cx| {
                        if let Some(surface) = *surface.borrow() {
                            let rect = HoverRect {
                                left: f32::from(area.left() - surface.left()),
                                top: f32::from(area.top() - surface.top()),
                                width: f32::from(area.size.width),
                                height: f32::from(area.size.height),
                            };
                            if hover.borrow_mut().measure("profile-host", rect) {
                                window.defer(cx, |window, _| window.refresh());
                            }
                        }
                        if bounds.replace(area) != area {
                            window.defer(cx, |window, _| window.refresh());
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .on_click(cx.listener(|app, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                app.open_machines(window, cx);
            }))
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(
                    event.keystroke.key.as_str(),
                    "enter" | "space" | "down" | "right"
                ) {
                    cx.stop_propagation();
                    app.open_machines(window, cx);
                }
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    let _ = app.profile_menu.dismiss();
                    app.begin_profile_menu_close(cx);
                    app.profile_focus.focus(window, cx);
                }
            }))
    }

    pub(super) fn activate_machine_hover(&self) {
        if let Some(entry) = self.machine_menu.entries.get(self.machine_menu.highlighted) {
            self.machine_menu
                .hover
                .borrow_mut()
                .set_active(entry.id.clone());
        }
    }

    pub(super) fn handle_machine_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let length = self.machine_menu.entries.len();
        match event.keystroke.key.as_str() {
            "escape" | "tab" | "left" => self.close_machines(window, cx),
            "down" if length > 0 => {
                self.machine_menu.highlighted = (self.machine_menu.highlighted + 1) % length;
            }
            "up" if length > 0 => {
                self.machine_menu.highlighted =
                    (self.machine_menu.highlighted + length - 1) % length;
            }
            "home" => self.machine_menu.highlighted = 0,
            "end" => self.machine_menu.highlighted = length.saturating_sub(1),
            "enter" | "space" => {
                self.choose_machine(self.machine_menu.highlighted, window, cx);
            }
            _ => return,
        }
        if self.machine_menu.open {
            self.activate_machine_hover();
            self.machine_menu
                .scroll
                .scroll_to_item(self.machine_menu.highlighted);
        }
        cx.stop_propagation();
        cx.notify();
    }
}
