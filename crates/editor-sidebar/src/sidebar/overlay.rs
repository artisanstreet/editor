use std::rc::Rc;

use aqua_core::NodeId;
use aqua_ui::overlay::OverlayId;
use artisan_api::ThreadId;

use super::{
    controller::{PickerIdentity, Sidebar},
    view::{self, ViewAction},
};
use editor_core::{Core, HostId, HostInstance};
use editor_ui::{
    dialog::DialogHost,
    popover::AnimatedPickerHost,
    text::mount::EditorController,
    theme::ArtisanTheme,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    host: HostId,
    instance: HostInstance,
    thread: ThreadId,
    generation: u64,
}

struct Menu {
    identity: Identity,
    popup: AnimatedPickerHost,
}

struct Rename {
    identity: Identity,
    dialog: DialogHost,
    editor: EditorController,
}

struct Picker {
    identity: PickerIdentity,
    overlay: OverlayId,
    creating: bool,
    emit: Rc<dyn Fn(ViewAction)>,
}

fn picker_root(
    dialog: editor_ui::dialog::Dialog,
    viewport: aqua_core::Size,
    theme: ArtisanTheme,
    identity: PickerIdentity,
    creating: bool,
    emit: Rc<dyn Fn(ViewAction)>,
) -> aqua_ui::element::Element {
    use aqua_core::{InputEvent, PointerButton};
    use aqua_ui::{
        AlignItems, JustifyContent,
        element::Box as UiBox,
        input::{EventPhase, InputKind, InputResponse, RoutedInput},
        style::Style,
    };
    use editor_ui::kit::surface::backdrop;
    let pressed = Rc::new(std::cell::Cell::new(false));
    let down = pressed.clone();
    let panel = dialog.build_panel(viewport);
    UiBox::new()
        .width(viewport.w)
        .height(viewport.h)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER),
        )
        .style(backdrop(&theme))
        .child(panel)
        .build()
        .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
            if !creating
                && event.target == event.current_target
                && matches!(
                    &event.input,
                    RoutedInput::Native(InputEvent::PointerDown(_, PointerButton::Primary))
                )
            {
                down.set(true);
            }
            InputResponse::default()
        })
        .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
            if pressed.replace(false)
                && !creating
                && event.target == event.current_target
                && matches!(
                    &event.input,
                    RoutedInput::Native(InputEvent::PointerUp(_, PointerButton::Primary))
                )
            {
                emit(ViewAction::DismissProjectPicker(identity.clone()));
            }
            InputResponse::default()
        })
}

#[derive(Default)]
pub struct SidebarOverlays {
    menu: Option<Menu>,
    rename: Option<Rename>,
    picker: Option<Picker>,
    retiring: Vec<AnimatedPickerHost>,
}

impl SidebarOverlays {
    pub fn sync(
        &mut self,
        app: &mut aqua::UiApp,
        owner: NodeId,
        sidebar: &Sidebar,
        core: &Core,
        theme: ArtisanTheme,
        now: f64,
        emit: Rc<dyn Fn(ViewAction)>,
    ) -> Result<(), String> {
        if let Some(rename) = &self.rename {
            rename.dialog.sync(app);
        }
        let host = sidebar.host_data(core);
        let menu_identity = host
            .zip(sidebar.menu.as_ref())
            .map(|(host, menu)| Identity {
                host: host.id.clone(),
                instance: host.instance(),
                thread: menu.thread.clone(),
                generation: sidebar.menu_generation,
            });
        if self.menu.as_ref().is_some_and(|menu| {
            Some(&menu.identity) != menu_identity.as_ref() || !menu.popup.is_open(app)
        }) {
            if let Some(mut menu) = self.menu.take() {
                if menu.popup.is_open(app) {
                    menu.popup.begin_close(app, now)?;
                    self.retiring.push(menu.popup);
                }
            }
        }
        if let Some(identity) = menu_identity {
            if let Some((element, options)) =
                view::thread_menu(sidebar, core, theme, app.viewport(), emit.clone())
            {
                if let Some(menu) = &mut self.menu {
                    menu.popup.refresh_content(app, element)?;
                } else {
                    let popup =
                        AnimatedPickerHost::open_options(app, owner, element, options, now)?;
                    self.menu = Some(Menu { identity, popup });
                }
            } else if let Some(mut menu) = self.menu.take() {
                if menu.popup.is_open(app) {
                    menu.popup.begin_close(app, now)?;
                    self.retiring.push(menu.popup);
                }
            }
        }
        let rename_identity = host
            .zip(sidebar.rename.as_ref())
            .map(|(host, thread)| Identity {
                host: host.id.clone(),
                instance: host.instance(),
                thread: thread.clone(),
                generation: sidebar.rename_generation,
            });
        if self.rename.as_ref().is_some_and(|rename| {
            Some(&rename.identity) != rename_identity.as_ref() || !rename.dialog.is_open(app)
        }) {
            if let Some(rename) = self.rename.take() {
                rename.dialog.dispose(app);
            }
        }
        if self.rename.is_none() {
            if let Some(identity) = rename_identity {
                if let Some(editor) = view::rename_editor(sidebar, core, emit.clone()) {
                    if let Some(dialog) =
                        view::rename_dialog(sidebar, core, theme, &editor, emit.clone())
                    {
                        let dialog = DialogHost::open(app, owner, dialog)?;
                        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
                            "sidebar-rename-input",
                        ));
                        if let Some(node) = app.tree().nodes_with_key(key).into_iter().next() {
                            app.tree_mut().set_focused(Some(node));
                        }
                        self.rename = Some(Rename {
                            identity,
                            dialog,
                            editor,
                        });
                    }
                }
            }
        }
        let picker_identity = sidebar.picker_identity(core);
        if self.picker.as_ref().is_some_and(|picker| {
            Some(&picker.identity) != picker_identity.as_ref()
                || !app.host().is_open(picker.overlay)
        }) {
            if let Some(picker) = self.picker.take() {
                app.close_popup(picker.overlay);
            }
        }
        if let Some(identity) = picker_identity {
            if let Some(dialog) = view::project_picker_dialog(sidebar, core, theme, emit.clone()) {
                let creating = sidebar
                    .visible_picker(core)
                    .is_some_and(|picker| picker.creating);
                let root = picker_root(
                    dialog,
                    app.viewport(),
                    theme,
                    identity.clone(),
                    creating,
                    emit.clone(),
                );
                if let Some(picker) = &mut self.picker {
                    app.refresh_popup(picker.overlay, root)
                        .map_err(|error| error.to_string())?;
                    picker.creating = creating;
                } else {
                    let mut options = aqua_ui::popup::PopupOptions::dialog();
                    options.placement.margin = 0.0;
                    options.focus = aqua_ui::popup::PopupFocus::Root;
                    options.dismiss = aqua_ui::overlay::DismissPolicy::MANUAL;
                    let overlay = app
                        .open_popup(owner, root, options)
                        .map_err(|error| error.to_string())?;
                    self.picker = Some(Picker {
                        identity,
                        overlay,
                        creating,
                        emit: emit.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn rename_editor(&self) -> Option<&EditorController> {
        self.rename.as_ref().map(|rename| &rename.editor)
    }

    pub fn request_dismissal(
        &mut self,
        app: &mut aqua::UiApp,
        event: &aqua_core::InputEvent,
    ) -> Result<bool, String> {
        if let Some(picker) = &self.picker {
            if app.host().service().topmost() == Some(picker.overlay)
                && matches!(event, aqua_core::InputEvent::KeyDown(key) if key.key == aqua_core::Key::Named(aqua_core::NamedKey::Escape))
            {
                if !picker.creating {
                    (picker.emit)(ViewAction::DismissProjectPicker(picker.identity.clone()));
                }
                return Ok(true);
            }
        }
        match &mut self.menu {
            Some(menu) => menu.popup.request_dismissal(app, event),
            None => Ok(false),
        }
    }

    pub fn advance(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<bool, String> {
        let mut animating = false;
        for popup in self
            .retiring
            .iter_mut()
            .chain(self.menu.as_mut().map(|entry| &mut entry.popup))
        {
            if popup.is_open(app) {
                let moving = popup.is_animating(app);
                popup.advance(app, now)?;
                animating |= moving || popup.is_animating(app);
            }
        }
        self.retiring.retain(|popup| popup.is_open(app));
        Ok(animating)
    }

    pub fn dispose(&mut self, app: &mut aqua::UiApp) {
        for popup in self.retiring.drain(..) {
            popup.dispose(app);
        }
        if let Some(picker) = self.picker.take() {
            app.close_popup(picker.overlay);
        }
        if let Some(menu) = self.menu.take() {
            menu.popup.dispose(app);
        }
        if let Some(rename) = self.rename.take() {
            rename.dialog.dispose(app);
        }
    }
}
