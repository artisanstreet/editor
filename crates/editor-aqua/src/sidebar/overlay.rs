use std::rc::Rc;

use aqua_core::NodeId;
use aqua_ui::overlay::OverlayId;
use artisan_api::ThreadId;

use super::{
    controller::{PickerIdentity, Sidebar},
    view::{self, ViewAction},
};
use crate::{
    core::{Core, HostId, HostInstance},
    ui::{
        dialog::DialogHost,
        popover::{AnimatedPickerHost, PickerEntrance},
        text::mount::EditorController,
        theme::ArtisanTheme,
    },
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
    overlay: OverlayId,
}

struct Rename {
    identity: Identity,
    dialog: DialogHost,
    editor: EditorController,
}

struct Profile {
    generation: u64,
    close_generation: Rc<std::cell::Cell<u64>>,
    popup: AnimatedPickerHost,
}

struct Anchored {
    owner: NodeId,
    generation: u64,
    overlay: OverlayId,
}

struct Tooltip {
    owner: NodeId,
    key: (artisan_api::EngineId, usize),
    overlay: OverlayId,
}

struct Picker {
    identity: PickerIdentity,
    overlay: OverlayId,
    creating: bool,
    emit: Rc<dyn Fn(ViewAction)>,
}

fn picker_root(
    dialog: crate::ui::dialog::Dialog,
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
    let pressed = Rc::new(std::cell::Cell::new(false));
    let down = pressed.clone();
    let panel = dialog.build_panel(viewport);
    UiBox::new()
        .width(viewport.w)
        .height(viewport.h)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .justify(JustifyContent::CENTER)
                .gpui_background(aqua_core::GpuiBackground::Solid(
                    theme
                        .colors
                        .background
                        .at(crate::ui::theme::Opacity::Backdrop)
                        .to_reference_paint(),
                )),
        )
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

fn advance_hover(
    hover: &mut crate::ui::picker_motion::HoverSurface,
    app: &mut aqua::UiApp,
    surface: Option<NodeId>,
    theme: &ArtisanTheme,
    reduced_motion: bool,
    now: f64,
) -> Result<bool, String> {
    match surface {
        Some(surface) => hover.advance(app, surface, theme, true, now, reduced_motion),
        None => {
            hover.dispose(app);
            Ok(false)
        }
    }
}

fn keyed_node(app: &aqua::UiApp, root: NodeId, key: String) -> Option<NodeId> {
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key));
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        let record = app.tree().get(node)?;
        if record.key == Some(key) {
            return Some(node);
        }
        pending.extend(record.children.iter().copied());
    }
    None
}

#[derive(Default)]
pub struct SidebarOverlays {
    menu: Option<Menu>,
    rename: Option<Rename>,
    profile: Option<Profile>,
    machines: Option<Anchored>,
    tooltip: Option<Tooltip>,
    picker: Option<Picker>,
}

impl SidebarOverlays {
    pub fn sync(
        &mut self,
        app: &mut aqua::UiApp,
        owner: NodeId,
        profile_owner: Option<NodeId>,
        sidebar: &Sidebar,
        core: &Core,
        theme: ArtisanTheme,
        now: f64,
        emit: Rc<dyn Fn(ViewAction)>,
    ) -> Result<(), String> {
        let usage_height = profile_owner
            .and_then(|node| app.tree().visual_rect(node))
            .map_or(0.0, |bounds| {
                (bounds.y.min(app.viewport().h) - 178.0).max(0.0)
            });
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
            Some(&menu.identity) != menu_identity.as_ref() || !app.host().is_open(menu.overlay)
        }) {
            if let Some(menu) = self.menu.take() {
                app.close_popup(menu.overlay);
            }
        }
        if let Some(identity) = menu_identity {
            if let Some((element, options)) =
                view::thread_menu(sidebar, core, theme, app.viewport(), emit.clone())
            {
                if let Some(menu) = &self.menu {
                    app.refresh_popup(menu.overlay, element)
                        .map_err(|error| error.to_string())?;
                } else {
                    let overlay = app
                        .open_popup_measured(owner, element, options)
                        .map_err(|error| error.to_string())?;
                    self.menu = Some(Menu { identity, overlay });
                }
            } else if let Some(menu) = self.menu.take() {
                app.close_popup(menu.overlay);
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
        if let Some(profile) = &mut self.profile {
            if profile.generation != sidebar.profile_generation
                && sidebar.profile_open
                && profile.popup.is_open(app)
            {
                profile.generation = sidebar.profile_generation;
                profile.close_generation.set(sidebar.profile_generation);
                profile.popup.begin_open(app, now)?;
            }
        }
        if self
            .profile
            .as_ref()
            .is_some_and(|profile| profile.generation != sidebar.profile_generation)
        {
            if let Some(profile) = self.profile.take() {
                profile.popup.dispose(app);
            }
        }
        if let Some(profile) = &mut self.profile {
            if !profile.popup.is_open(app) {
                self.profile = None;
            } else if !sidebar.profile_open && !profile.popup.is_closing() {
                profile.popup.begin_close(app, now)?;
            } else if sidebar.profile_open && !profile.popup.is_closing() {
                profile.popup.refresh_content(
                    app,
                    super::profile::content(sidebar, core, theme, usage_height, emit.clone()),
                )?;
            }
        }
        if self.profile.is_none() && sidebar.profile_open {
            if let Some(profile_owner) = profile_owner {
                let generation = sidebar.profile_generation;
                let close_generation = Rc::new(std::cell::Cell::new(generation));
                let captured_generation = close_generation.clone();
                let close = emit.clone();
                let content =
                    super::profile::content(sidebar, core, theme, usage_height, emit.clone());
                let popup = AnimatedPickerHost::open_content(
                    app,
                    profile_owner,
                    content,
                    4.0,
                    aqua_ui::popup::PopupFocus::Preserve,
                    PickerEntrance::Scale {
                        origin: aqua_core::Point::new(0.0, 1.0),
                    },
                    std::time::Duration::from_millis(250),
                    std::time::Duration::from_millis(150),
                    now,
                    move |_| {
                        close(ViewAction::CloseProfileFor {
                            generation: captured_generation.get(),
                        })
                    },
                )?;
                self.profile = Some(Profile {
                    generation,
                    close_generation,
                    popup,
                });
            }
        }
        let profile_root = self.profile_root(app).filter(|_| sidebar.profile_open);
        let machine_owner = profile_root
            .and_then(|root| keyed_node(app, root, "artisan-desktop-profile-machine".into()));
        let wanted_machine = machine_owner.filter(|_| sidebar.profile_machines_open);
        if self.machines.as_ref().is_some_and(|entry| {
            Some(entry.owner) != wanted_machine
                || entry.generation != sidebar.machines_generation
                || !app.host().is_open(entry.overlay)
        }) {
            if let Some(entry) = self.machines.take() {
                app.close_popup(entry.overlay);
            }
        }
        if let Some(machine_owner) = wanted_machine {
            let element =
                super::profile::machines(sidebar, core, theme, app.viewport().w, emit.clone());
            if let Some(entry) = &self.machines {
                app.refresh_popup(entry.overlay, element)
                    .map_err(|error| error.to_string())?;
            } else {
                let mut options = aqua_ui::popup::PopupOptions::menu();
                options.focus = aqua_ui::popup::PopupFocus::Preserve;
                options.placement.preferred = aqua_ui::overlay::Side::After;
                options.placement.align = aqua_ui::overlay::Align::Start;
                options.placement.gap = 6.0;
                let generation = sidebar.profile_generation;
                let machines_generation = sidebar.machines_generation;
                let close = emit.clone();
                options.on_close = Some(Rc::new(move |_| {
                    close(ViewAction::CloseProfileMachinesFor {
                        generation,
                        machines_generation,
                    })
                }));
                let overlay = app
                    .open_popup_measured(machine_owner, element, options)
                    .map_err(|error| error.to_string())?;
                self.machines = Some(Anchored {
                    owner: machine_owner,
                    generation: machines_generation,
                    overlay,
                });
            }
        }
        let wanted_tooltip = sidebar.usage_hover.and_then(|key| {
            profile_root
                .and_then(|root| {
                    keyed_node(app, root, format!("artisan-profile-usage-meter-{}", key.0))
                })
                .map(|owner| (owner, key))
        });
        if self.tooltip.as_ref().is_some_and(|entry| {
            Some((entry.owner, entry.key)) != wanted_tooltip || !app.host().is_open(entry.overlay)
        }) {
            if let Some(entry) = self.tooltip.take() {
                app.close_popup(entry.overlay);
            }
        }
        if let Some((tooltip_owner, key)) = wanted_tooltip {
            if let Some(element) =
                super::usage_view::tooltip_panel(sidebar, core, theme, artisan_api::now())
            {
                if let Some(entry) = &self.tooltip {
                    app.refresh_popup(entry.overlay, element)
                        .map_err(|error| error.to_string())?;
                } else {
                    let mut options = aqua_ui::popup::PopupOptions::tooltip();
                    options.dismiss = aqua_ui::overlay::DismissPolicy::MANUAL;
                    options.placement.preferred = aqua_ui::overlay::Side::After;
                    options.placement.align = aqua_ui::overlay::Align::Start;
                    options.placement.gap = 8.0;
                    options.placement.flip = false;
                    let overlay = app
                        .open_popup_measured(tooltip_owner, element, options)
                        .map_err(|error| error.to_string())?;
                    let _ = app.set_popup_uncontained(overlay, true);
                    self.tooltip = Some(Tooltip {
                        owner: tooltip_owner,
                        key,
                        overlay,
                    });
                }
            }
        }
        Ok(())
    }

    pub fn rename_editor(&self) -> Option<&EditorController> {
        self.rename.as_ref().map(|rename| &rename.editor)
    }

    pub fn profile_root(&self, app: &aqua::UiApp) -> Option<NodeId> {
        self.profile.as_ref()?.popup.root(app)
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
        match &mut self.profile {
            Some(profile) => profile.popup.request_dismissal(app, event),
            None => Ok(false),
        }
    }

    pub fn advance(
        &mut self,
        app: &mut aqua::UiApp,
        sidebar: &mut Sidebar,
        theme: &ArtisanTheme,
        reduced_motion: bool,
        now: f64,
    ) -> Result<bool, String> {
        let mut animating = false;
        if let Some(profile) = &mut self.profile {
            if profile.popup.is_open(app) {
                let was_animating = profile.popup.is_animating(app);
                profile.popup.advance(app, now)?;
                animating |= was_animating || profile.popup.is_animating(app);
            }
        }
        let profile_surface = self
            .profile_root(app)
            .and_then(|root| keyed_node(app, root, "artisan-profile-content".into()));
        animating |= advance_hover(
            &mut sidebar.profile_hover,
            app,
            profile_surface,
            theme,
            reduced_motion,
            now,
        )?;
        let machines_surface = self
            .machines
            .as_ref()
            .filter(|entry| app.host().is_open(entry.overlay))
            .and_then(|entry| {
                app.host().popup_root(entry.overlay)
                    .and_then(|root| keyed_node(app, root, "machines-hover-surface".into()))
            });
        animating |= advance_hover(
            &mut sidebar.machines_hover,
            app,
            machines_surface,
            theme,
            reduced_motion,
            now,
        )?;
        Ok(animating)
    }

    pub fn dispose(&mut self, app: &mut aqua::UiApp) {
        if let Some(picker) = self.picker.take() {
            app.close_popup(picker.overlay);
        }
        if let Some(entry) = self.tooltip.take() {
            app.close_popup(entry.overlay);
        }
        if let Some(entry) = self.machines.take() {
            app.close_popup(entry.overlay);
        }
        if let Some(menu) = self.menu.take() {
            app.close_popup(menu.overlay);
        }
        if let Some(rename) = self.rename.take() {
            rename.dialog.dispose(app);
        }
        if let Some(profile) = self.profile.take() {
            profile.popup.dispose(app);
        }
    }
}
