use std::rc::Rc;

use aqua_core::{Point, Size};
use aqua_ui::{
    element::{Box as UiBox, Element},
    popup::PopupOptions,
    style::{Length, Style},
};
use artisan_api::{ProjectId, ThreadId};
use editor_assets::AssetId;

use super::{
    controller::{PickerIdentity, Sidebar, ThreadMenu},
    row::{MenuAction, menu_actions},
};
use editor_core::{Core, HostId, HostInstance, Screen};
use editor_ui::{
    kit::{
        Button, ButtonTone, Density, Divider, Hover, IconSize, Row, RowItem, Space, Spaced,
        Surface, TextRole,
    },
    popover::Popover,
    theme::ArtisanTheme,
};

const THREAD_MENU_WIDTH: f32 = 198.0;
const FOLDER_ROW: Row = Row::ListItem(Density::Regular);
const FOLDER_LIST_HEIGHT: f32 = 320.0;
const COLUMN_GAP: Space = Space::S12;

#[derive(Clone, Debug)]
pub enum ViewAction {
    OpenThread {
        host: HostId,
        instance: HostInstance,
        thread: ThreadId,
    },
    NewThread(Option<ProjectId>),
    Search,
    OpenHit {
        host: HostId,
        instance: HostInstance,
        hit: super::list::HitRow,
    },
    Navigate(Screen),
    OpenThreadMenu {
        host: HostId,
        instance: HostInstance,
        thread: ThreadId,
        position: Point,
    },
    DismissThreadMenu,
    DismissThreadMenuFor {
        host: HostId,
        instance: HostInstance,
        generation: u64,
    },
    ThreadMenuAction {
        host: HostId,
        instance: HostInstance,
        thread: ThreadId,
        generation: u64,
        action: MenuAction,
    },
    SubmitRename(String),
    SubmitRenameFor {
        host: HostId,
        instance: HostInstance,
        thread: ThreadId,
        generation: u64,
        text: String,
    },
    DismissRename,
    DismissRenameFor {
        host: HostId,
        instance: HostInstance,
        thread: ThreadId,
        generation: u64,
    },
    ToggleHistory,
    AddProject,
    BrowseFolder {
        picker: PickerIdentity,
        path: Option<String>,
    },
    CreateProject(PickerIdentity),
    DismissProjectPicker(PickerIdentity),
    RetryProjectIcons,
    DismissError,
    RailHover(super::rail::Glyph),
    SetDarkMode(bool),
    ThreadViewport {
        host: HostId,
        instance: HostInstance,
        viewport: aqua_ui::collection_runtime::CollectionViewport,
        visible_projects: Vec<ProjectId>,
    },
    ThreadWheel {
        host: HostId,
        instance: HostInstance,
    },
    ThreadDragStarted {
        payload: editor_core::DraggedThread,
        instance: HostInstance,
        position: Point,
    },
    ThreadDragEnded {
        host: HostId,
        instance: HostInstance,
        thread: ThreadId,
    },
}

fn visible_load_state(sidebar: &Sidebar, core: &Core) -> Option<(Option<String>, bool)> {
    sidebar
        .host_data(core)
        .filter(|host| host.is_live())
        .map(|host| {
            (
                host.thread_page(false).error.clone(),
                host.project_icons_failed(),
            )
        })
}

pub(super) fn visible_state(sidebar: &Sidebar, core: &Core) -> impl PartialEq + use<> {
    (
        (
            core.screen().clone(),
            core.workspace_screen().clone(),
            core.settings.get("dark_mode").cloned(),
        ),
        (
            sidebar.menu.clone(),
            sidebar.menu.as_ref().map(|_| sidebar.menu_generation),
            sidebar.rename.clone(),
            sidebar.rename.as_ref().map(|_| sidebar.rename_generation),
            sidebar.error.clone(),
        ),
        sidebar.history_open,
        (
            sidebar.host.clone(),
            sidebar.open.clone(),
            sidebar.lists.clone(),
        ),
        visible_load_state(sidebar, core),
        sidebar.picker.as_ref().map(|picker| {
            (
                picker.listing.clone(),
                picker.hidden,
                picker.loading,
                picker.creating,
                picker.error.clone(),
            )
        }),
    )
}

pub fn dispatch(sidebar: &mut Sidebar, core: &mut Core, action: ViewAction) -> bool {
    match &action {
        ViewAction::ThreadViewport {
            host,
            instance,
            viewport,
            visible_projects,
        } => {
            let before = visible_load_state(sidebar, core);
            sidebar.report_thread_viewport(core, host, *instance, *viewport, visible_projects);
            return before != visible_load_state(sidebar, core);
        }
        ViewAction::ThreadWheel { host, instance } => {
            let before = visible_load_state(sidebar, core);
            if sidebar
                .host_data(core)
                .map(|current| (&current.id, current.instance()))
                == Some((host, *instance))
            {
                sidebar.page_at_viewport(core, true);
            }
            return before != visible_load_state(sidebar, core);
        }
        ViewAction::ThreadDragStarted {
            payload,
            instance,
            position,
        } => {
            if sidebar
                .host_data(core)
                .map(|host| (&host.id, host.instance()))
                == Some((&payload.host, *instance))
            {
                sidebar.drag = Some((payload.host.clone(), *instance, payload.thread.clone()));
                sidebar
                    .effects
                    .push(super::controller::SidebarEffect::ShowDragPreview {
                        payload: payload.clone(),
                        position: *position,
                    });
            }
            return false;
        }
        ViewAction::ThreadDragEnded {
            host,
            instance,
            thread,
        } => {
            if sidebar.drag.as_ref() == Some(&(host.clone(), *instance, thread.clone())) {
                sidebar.drag = None;
                sidebar
                    .effects
                    .push(super::controller::SidebarEffect::HideDragPreview);
            }
            return false;
        }
        _ => {}
    }
    let before = visible_state(sidebar, core);
    let mut animation_started = false;
    match action {
        ViewAction::OpenThread {
            host,
            instance,
            thread,
        } => {
            if sidebar
                .host_data(core)
                .map(|current| (&current.id, current.instance()))
                == Some((&host, instance))
            {
                sidebar.open_thread(core, thread);
            }
        }
        ViewAction::NewThread(project) => sidebar.new_thread(core, project),
        ViewAction::Search => {
            sidebar.search_text(core);
            sidebar.sync(core, artisan_api::now());
        }
        ViewAction::OpenHit {
            host,
            instance,
            hit,
        } => {
            if sidebar
                .host_data(core)
                .map(|current| (&current.id, current.instance()))
                == Some((&host, instance))
            {
                sidebar.open_thread(core, hit.thread.clone());
                sidebar
                    .effects
                    .push(super::controller::SidebarEffect::Reveal(hit));
            }
        }
        ViewAction::Navigate(screen) => core.navigate(screen),
        ViewAction::OpenThreadMenu {
            host,
            instance,
            thread,
            position,
        } => {
            if sidebar
                .host_data(core)
                .map(|current| (&current.id, current.instance()))
                == Some((&host, instance))
            {
                sidebar.menu_generation = sidebar.menu_generation.wrapping_add(1);
                sidebar.menu = Some(ThreadMenu { thread, position });
            }
        }
        ViewAction::DismissThreadMenu => sidebar.menu = None,
        ViewAction::DismissThreadMenuFor {
            host,
            instance,
            generation,
        } => {
            if sidebar.menu_generation == generation
                && sidebar
                    .host_data(core)
                    .map(|current| (&current.id, current.instance()))
                    == Some((&host, instance))
            {
                sidebar.menu = None;
            }
        }
        ViewAction::ThreadMenuAction {
            host,
            instance,
            thread,
            generation,
            action,
        } => {
            if sidebar
                .host_data(core)
                .map(|current| (&current.id, current.instance()))
                != Some((&host, instance))
                || core.active_host().map(|current| current.instance()) != Some(instance)
                || sidebar.menu_generation != generation
                || sidebar.menu.as_ref().map(|menu| &menu.thread) != Some(&thread)
            {
                return false;
            }
            sidebar.menu = None;
            match action {
                MenuAction::Settle => sidebar.settle(core, thread),
                MenuAction::Rename => {
                    sidebar.rename_generation = sidebar.rename_generation.wrapping_add(1);
                    sidebar.rename = Some(thread);
                }
                MenuAction::MarkUnread => sidebar.mark_unread(core, thread),
                MenuAction::Archive => sidebar.archive(core, thread),
            }
        }
        ViewAction::SubmitRename(text) => sidebar.submit_rename(core, &text),
        ViewAction::SubmitRenameFor {
            host,
            instance,
            thread,
            generation,
            text,
        } => {
            if sidebar.rename.as_ref() == Some(&thread)
                && sidebar.rename_generation == generation
                && sidebar
                    .host_data(core)
                    .map(|current| (&current.id, current.instance()))
                    == Some((&host, instance))
            {
                sidebar.submit_rename(core, &text);
            }
        }
        ViewAction::DismissRename => sidebar.rename = None,
        ViewAction::DismissRenameFor {
            host,
            instance,
            thread,
            generation,
        } => {
            if sidebar.rename_generation == generation
                && sidebar.rename.as_ref() == Some(&thread)
                && sidebar
                    .host_data(core)
                    .map(|current| (&current.id, current.instance()))
                    == Some((&host, instance))
            {
                sidebar.rename = None;
            }
        }
        ViewAction::ToggleHistory => sidebar.toggle_history(core),
        ViewAction::AddProject => sidebar.add_project(core),
        ViewAction::BrowseFolder { picker, path } => {
            if sidebar.picker_identity(core).as_ref() == Some(&picker) {
                sidebar.browse(core, path);
            }
        }
        ViewAction::CreateProject(picker) => {
            if sidebar.picker_identity(core).as_ref() == Some(&picker) {
                sidebar.create_project(core);
            }
        }
        ViewAction::DismissProjectPicker(picker) => {
            if sidebar.picker_identity(core).as_ref() == Some(&picker) {
                sidebar.dismiss_picker();
            }
        }
        ViewAction::RetryProjectIcons => sidebar.retry_project_icons(core),
        ViewAction::DismissError => sidebar.error = None,
        ViewAction::RailHover(glyph) => {
            animation_started = sidebar.rail_motion.play(glyph, std::time::Instant::now());
        }
        ViewAction::SetDarkMode(dark) => core.set_setting("dark_mode", dark.into()),
        ViewAction::ThreadViewport {
            host,
            instance,
            viewport,
            visible_projects,
        } => {
            sidebar.report_thread_viewport(core, &host, instance, viewport, &visible_projects);
        }
        ViewAction::ThreadWheel { host, instance } => {
            if sidebar
                .host_data(core)
                .map(|current| (&current.id, current.instance()))
                == Some((&host, instance))
            {
                sidebar.page_at_viewport(core, true);
            }
        }
        ViewAction::ThreadDragStarted { .. } | ViewAction::ThreadDragEnded { .. } => {}
    }
    animation_started || before != visible_state(sidebar, core)
}

pub fn thread_menu(
    sidebar: &Sidebar,
    core: &Core,
    theme: ArtisanTheme,
    viewport: Size,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<(Element, PopupOptions)> {
    let menu = sidebar.menu.as_ref()?;
    let host = sidebar.host_data(core)?;
    let thread = host.threads.get(&menu.thread)?;
    let row = sidebar
        .lists
        .inbox
        .iter()
        .chain(
            sidebar
                .lists
                .history
                .iter()
                .flat_map(|group| group.rows.iter()),
        )
        .find(|row| row.id == menu.thread)?;
    let actions = menu_actions(row, thread);
    let mut content = UiBox::new()
        .layout("column")
        .style(Style::new().width(Length::Px(THREAD_MENU_WIDTH)));
    for action in actions {
        if action == MenuAction::Archive || (action == MenuAction::Rename && row.settleable) {
            content = content.child(Divider::Rule.build(&theme, Surface::Menu.padding_px()));
        }
        let (id, icon, label, hover) = match action {
            MenuAction::Settle => (
                "sidebar-menu-settle",
                AssetId::TABLER_CIRCLE_CHECK,
                "Settle Thread",
                Hover::Neutral,
            ),
            MenuAction::Rename => (
                "sidebar-menu-rename",
                AssetId::TABLER_PENCIL,
                "Rename Thread",
                Hover::Neutral,
            ),
            MenuAction::MarkUnread => (
                "sidebar-menu-unread",
                AssetId::TABLER_BELL,
                "Mark as Unread",
                Hover::Neutral,
            ),
            MenuAction::Archive => (
                "sidebar-menu-archive",
                AssetId::TABLER_ARCHIVE,
                "Archive Thread",
                Hover::Destructive,
            ),
        };
        let icon_color = match hover {
            Hover::Destructive => theme.colors.destructive_text,
            Hover::Neutral => theme.colors.text_muted,
        };
        let selected_thread = menu.thread.clone();
        let selected_host = host.id.clone();
        let selected_instance = host.instance();
        let generation = sidebar.menu_generation;
        let selected_emit = emit.clone();
        let item = RowItem::new(id, theme, Row::MenuItem, Surface::Menu, label)
            .trailing(IconSize::Md.glyph(icon, icon_color.to_reference_paint()))
            .on_activate(move || {
                selected_emit(ViewAction::ThreadMenuAction {
                    host: selected_host.clone(),
                    instance: selected_instance,
                    thread: selected_thread.clone(),
                    generation,
                    action,
                })
            });
        content = content.child(match hover {
            Hover::Destructive => item.destructive().build(),
            Hover::Neutral => item.build(),
        });
    }
    let dismiss = emit.clone();
    let dismiss_host = host.id.clone();
    let dismiss_instance = host.instance();
    let dismiss_generation = sidebar.menu_generation;
    Some(
        Popover::new(
            "sidebar-menu",
            "Thread actions",
            menu.position,
            theme,
            content.build().with_key("thread-menu-hover-surface"),
            viewport,
            move || {
                dismiss(ViewAction::DismissThreadMenuFor {
                    host: dismiss_host.clone(),
                    instance: dismiss_instance,
                    generation: dismiss_generation,
                })
            },
        )
        .into_parts(),
    )
}

pub fn rename_editor(
    sidebar: &Sidebar,
    core: &Core,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<editor_ui::text::mount::EditorController> {
    use editor_ui::text::{
        Editor, Mode,
        mount::{EditorController, EditorEvent},
    };

    let thread = sidebar.rename.clone()?;
    let host = sidebar.host_data(core)?;
    let title = host
        .threads
        .get(&thread)
        .and_then(|thread| thread.title.as_deref())
        .unwrap_or_default();
    let mut editor = Editor::new(title, Mode::SingleLine);
    editor.select_all();
    let state = Rc::new(std::cell::RefCell::new(editor));
    let submitted = state.clone();
    let host_id = host.id.clone();
    let instance = host.instance();
    let generation = sidebar.rename_generation;
    Some(EditorController::from_shared(state, move |event| {
        if event == EditorEvent::Submitted {
            emit(ViewAction::SubmitRenameFor {
                host: host_id.clone(),
                instance,
                thread: thread.clone(),
                generation,
                text: submitted.borrow().text().to_owned(),
            });
        }
    }))
}

pub fn search_editor(
    sidebar: &Sidebar,
    theme: &ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    use editor_ui::text::{
        Mode,
        mount::{EditorController, EditorEvent},
    };

    EditorController::from_shared(Rc::clone(&sidebar.search), move |event| {
        if event == EditorEvent::Changed {
            emit(ViewAction::Search);
        }
    })
    .element_with_notice_keyed(
        "sidebar-thread-search",
        Mode::SingleLine,
        theme,
        "Search threads",
        true,
        false,
    )
}

pub fn rename_dialog(
    sidebar: &Sidebar,
    core: &Core,
    theme: ArtisanTheme,
    editor: &editor_ui::text::mount::EditorController,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<editor_ui::dialog::Dialog> {
    let thread = sidebar.rename.clone()?;
    let host = sidebar.host_data(core)?;
    let host_id = host.id.clone();
    let instance = host.instance();
    let generation = sidebar.rename_generation;
    let content = editor.element_with_notice_keyed(
        "sidebar-rename-input",
        editor_ui::text::Mode::SingleLine,
        &theme,
        "Thread title",
        false,
        true,
    );
    Some(
        editor_ui::dialog::Dialog::new("sidebar-rename", theme, "Rename Thread", content)
            .on_dismiss(move |_| {
                emit(ViewAction::DismissRenameFor {
                    host: host_id.clone(),
                    instance,
                    thread: thread.clone(),
                    generation,
                })
            }),
    )
}

pub fn project_picker_dialog(
    sidebar: &Sidebar,
    core: &Core,
    theme: ArtisanTheme,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<editor_ui::dialog::Dialog> {
    use aqua_ui::{JustifyContent, collection::List};

    let picker = sidebar.visible_picker(core)?;
    let identity = sidebar.picker_identity(core)?;
    let busy = picker.loading || picker.creating;
    let mut rows = Vec::new();
    if let Some(listing) = &picker.listing {
        if let Some(parent) = &listing.parent {
            rows.push((parent.clone(), "..".to_owned(), AssetId::TABLER_ARROW_UP));
        }
        rows.extend(picker.rows().into_iter().map(|index| {
            let entry = &listing.entries[index];
            (
                entry.path.clone(),
                entry.name.clone(),
                AssetId::TABLER_FOLDER,
            )
        }));
    }
    let row_emit = emit.clone();
    let row_picker = identity.clone();
    let list = List::new()
        .items(rows)
        .key(|(path, _, _)| format!("folder-{path}"))
        .row(move |(path, label, icon)| {
            let selected_path = path.clone();
            let action = row_emit.clone();
            let picker_identity = row_picker.clone();
            let row = RowItem::new(
                format!("folder-row-{path}"),
                theme,
                FOLDER_ROW,
                Surface::Inset,
                label.clone(),
            )
            .leading(IconSize::Md.glyph(*icon, theme.colors.text_muted.to_reference_paint()))
            .disabled(busy);
            match busy {
                true => row.build(),
                false => row
                    .on_activate(move || {
                        action(ViewAction::BrowseFolder {
                            picker: picker_identity.clone(),
                            path: Some(selected_path.clone()),
                        })
                    })
                    .build(),
            }
        })
        .style(
            Style::new()
                .height(Length::Px(FOLDER_LIST_HEIGHT))
                .width(Length::Pct(100.0)),
        )
        .estimated_row_height(Length::Px(FOLDER_ROW.height().unwrap_or_default()))
        .virtualize()
        .build();
    let path = picker
        .listing
        .as_ref()
        .map_or("Loading…", |listing| listing.path.as_str());
    let mut content = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .spaced(Space::S12)
        .child(
            TextRole::Label
                .text(&theme, path.to_owned())
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .build(),
        )
        .child(Surface::Inset.build(&theme, vec![list]));
    if let Some(error) = &picker.error {
        content = content.child(
            TextRole::Label
                .text(&theme, error.clone())
                .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
                .build(),
        );
    }
    let can_add = !busy && picker.listing.is_some();
    let add = emit.clone();
    let add_picker = identity.clone();
    let add_button = Button::text(
        "sidebar-folder-add",
        theme,
        ButtonTone::Primary,
        if picker.creating {
            "Adding…"
        } else {
            "Add this folder"
        },
    )
    .disabled(!can_add)
    .on_activate(move |_| add(ViewAction::CreateProject(add_picker.clone())))
    .build();
    content = content.child(
        UiBox::new()
            .width(Length::Pct(100.0))
            .style(Style::new().justify(JustifyContent::END))
            .child(add_button)
            .build(),
    );
    let dismiss = emit;
    Some(
        editor_ui::dialog::Dialog::new(
            "sidebar-add-project",
            theme,
            "Add a project",
            content.build(),
        )
        .dismissible(!picker.creating)
        .on_dismiss(move |_| dismiss(ViewAction::DismissProjectPicker(identity.clone()))),
    )
}

pub const ROOT_KEY: &str = "sidebar-root";

pub fn thread_list(
    sidebar: &Sidebar,
    core: &Core,
    viewport_height: f32,
    render: Rc<dyn Fn(usize, &super::geometry::Entry, f32) -> Element>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    thread_entries(
        sidebar,
        core,
        viewport_height,
        super::geometry::entries(&sidebar.lists, sidebar.history_visible()),
        "sidebar-threads",
        render,
        emit,
    )
}

fn thread_entries(
    sidebar: &Sidebar,
    core: &Core,
    viewport_height: f32,
    entries: Vec<super::geometry::Entry>,
    scroller: &'static str,
    render: Rc<dyn Fn(usize, &super::geometry::Entry, f32) -> Element>,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    use super::geometry::{Entry, READ_AHEAD, entry_keys, history_spacer};
    use aqua_ui::{
        collection::List,
        input::{EventPhase, InputKind, InputResponse},
    };

    let host = sidebar.host_data(core)?;
    let host_id = host.id.clone();
    let instance = host.instance();
    let spacer = history_spacer(&entries, viewport_height, sidebar.history_fraction);
    let heights: Vec<_> = entries
        .iter()
        .map(|entry| match entry {
            Entry::Spacer => spacer,
            _ => entry.height(),
        })
        .collect();
    let keys = entry_keys(&entries);
    let items: Vec<_> = keys
        .into_iter()
        .zip(entries.iter().cloned())
        .enumerate()
        .map(|(index, (key, entry))| (key, index, entry))
        .collect();
    let viewport_entries = Rc::new(entries);
    let viewport_emit = emit.clone();
    let viewport_host = host_id.clone();
    let wheel_emit = emit;
    let fraction = sidebar.history_fraction;
    Some(
        List::new()
            .items(items)
            .key(|(key, _, _)| key.clone())
            .row(move |(_, index, entry)| render(*index, entry, fraction))
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0)),
            )
            .estimated_row_height(Length::Px(super::geometry::ThreadLayout::Standalone.height()))
            .initial_height(move |index| heights.get(index).copied().unwrap_or(0.0))
            .virtualize()
            .overscan_pixels(READ_AHEAD)
            .on_viewport(move |viewport| {
                let start = viewport.visible.start.min(viewport_entries.len());
                let end = viewport.visible.end.min(viewport_entries.len()).max(start);
                let visible_projects = viewport_entries[start..end]
                    .iter()
                    .filter_map(|entry| match entry {
                        Entry::Thread(row, _) => Some(row.project.clone()),
                        _ => None,
                    })
                    .collect();
                viewport_emit(ViewAction::ThreadViewport {
                    host: viewport_host.clone(),
                    instance,
                    viewport,
                    visible_projects,
                });
            })
            .build()
            .with_key(scroller)
            .on_input(InputKind::Scroll, EventPhase::Bubble, move |_| {
                wheel_emit(ViewAction::ThreadWheel {
                    host: host_id.clone(),
                    instance,
                });
                InputResponse::default()
            }),
    )
}

pub type ThreadVisualFactory = Rc<
    dyn Fn(
        &super::list::RowData,
        Option<std::sync::Arc<aqua::images::ImageHandle>>,
        Option<Option<String>>,
    ) -> super::row::RowVisuals,
>;

#[derive(Clone)]
struct ThreadMetadata {
    icon: Option<std::sync::Arc<aqua::images::ImageHandle>>,
    repository_host: Option<Option<String>>,
    drag: Option<editor_core::DraggedThread>,
}

pub fn thread_list_view(
    sidebar: &Sidebar,
    core: &Core,
    theme: ArtisanTheme,
    viewport_height: f32,
    visuals: ThreadVisualFactory,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    thread_entries_view(
        sidebar,
        core,
        theme,
        viewport_height,
        super::geometry::entries(&sidebar.lists, sidebar.history_visible()),
        "sidebar-threads",
        visuals,
        emit,
    )
}

pub fn thread_entries_view(
    sidebar: &Sidebar,
    core: &Core,
    theme: ArtisanTheme,
    viewport_height: f32,
    entries: Vec<super::geometry::Entry>,
    scroller: &'static str,
    visuals: ThreadVisualFactory,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Option<Element> {
    use super::geometry::{Entry, history_spacer};
    use aqua_core::Transform2D;
    use std::collections::HashMap;

    let host = sidebar.host_data(core)?;
    let host_id = host.id.clone();
    let instance = host.instance();
    let history_index = entries
        .iter()
        .position(|entry| matches!(entry, Entry::History));
    let spacer = history_spacer(&entries, viewport_height, sidebar.history_fraction);
    let metadata: HashMap<_, _> = entries
        .iter()
        .filter_map(|entry| {
            let Entry::Thread(row, _) = entry else {
                return None;
            };
            let thread = host.threads.get(&row.id)?;
            Some((
                row.id.clone(),
                ThreadMetadata {
                    icon: host.project_icon(&row.project),
                    repository_host: host
                        .repository(&row.project)
                        .map(|repository| repository.host.clone()),
                    drag: super::row::drag(host.id.clone(), thread),
                },
            ))
        })
        .collect();
    let project_marks: HashMap<_, _> = entries.iter().filter_map(|entry| {
        let Entry::Project { id, .. } = entry else { return None; };
        Some((id.clone(), editor_ui::project_mark::Mark::of(host, id)))
    }).collect();
    let row_entries = entries.clone();
    let row_emit = emit.clone();
    let row_host = host_id;
    let renderer = Rc::new(move |index: usize, entry: &Entry, fraction: f32| {
        let content = match entry {
            Entry::Thread(data, layout) => {
                let meta = metadata.get(&data.id).cloned().unwrap_or(ThreadMetadata {
                    icon: None,
                    repository_host: None,
                    drag: None,
                });
                let row_visuals = visuals(data, meta.icon, meta.repository_host);
                super::row::build(
                    data,
                    row_host.clone(),
                    instance,
                    meta.drag,
                    row_visuals,
                    *layout,
                    theme,
                    row_emit.clone(),
                )
            }
            Entry::Hit(data) => {
                let emit = row_emit.clone();
                let host = row_host.clone();
                let hit = data.clone();
                super::row::hit(theme, data, move || {
                    emit(ViewAction::OpenHit {
                        host: host.clone(),
                        instance,
                        hit: hit.clone(),
                    });
                })
            }
            Entry::Project { id, label } => {
                let row = RowItem::new(
                    format!("sidebar-project-{id}"),
                    theme,
                    super::geometry::HEADING,
                    super::geometry::LIST,
                    label.clone(),
                );
                match project_marks.get(id) {
                    Some(mark) => row.leading(mark.view(theme.colors.text_muted)).build(),
                    None => row.build(),
                }
            }
            Entry::Gap(height) => UiBox::new().height(*height).build(),
            Entry::Spacer => UiBox::new().height(spacer).build(),
            Entry::Heading(label) => RowItem::new(
                format!("sidebar-heading-{label}"),
                theme,
                super::geometry::HEADING,
                super::geometry::LIST,
                *label,
            )
            .build(),
            Entry::History => history_toggle(theme, fraction, row_emit.clone()),
        };
        if history_index.is_some_and(|history| index > history) {
            UiBox::new()
                .height(entry.height())
                .clip(true)
                .child(
                    UiBox::new()
                        .height(entry.height())
                        .opacity(fraction)
                        .style(Style::new().transform(Transform2D::translate(
                            0.0,
                            Space::S8.px() * (1.0 - fraction),
                        )))
                        .child(content)
                        .build(),
                )
                .build()
        } else {
            content
        }
    });
    thread_entries(
        sidebar,
        core,
        viewport_height,
        row_entries,
        scroller,
        renderer,
        emit,
    )
}

pub fn history_toggle(theme: ArtisanTheme, fraction: f32, emit: Rc<dyn Fn(ViewAction)>) -> Element {
    let toggle = emit;
    let caret = editor_ui::icon::disclosure_chevron(
        fraction,
        IconSize::Sm.px(),
        theme.colors.text_muted.to_reference_paint(),
    );
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .child(Divider::Rule.build(&theme, 0.0))
        .child(
            RowItem::new(
                "sidebar-history",
                theme,
                super::geometry::TOGGLE,
                super::geometry::LIST,
                "History",
            )
            .leading(IconSize::Md.glyph(
                AssetId::TABLER_ARCHIVE,
                theme.colors.text_muted.to_reference_paint(),
            ))
            .trailing(caret)
            .on_activate(move || toggle(ViewAction::ToggleHistory))
            .build(),
        )
        .build()
}

pub fn build(
    sidebar: &Sidebar,
    core: &Core,
    theme: ArtisanTheme,
    viewport: Size,
    visible: bool,
    reduced_motion: bool,
    now: std::time::Instant,
    visuals: ThreadVisualFactory,
    emit: Rc<dyn Fn(ViewAction)>,
) -> Element {
    if let Screen::Settings(page) = sidebar.screen(core) {
        let navigate = emit.clone();
        return super::settings_nav::build(
            sidebar.host_data(core),
            *page,
            theme,
            &sidebar.settings_motion,
            now,
            reduced_motion,
            Rc::new(move |page| navigate(ViewAction::Navigate(Screen::Settings(page)))),
        );
    }
    let rail = super::rail::build(sidebar, core, theme, now, emit.clone());
    let mut column = UiBox::new()
        .layout("column")
        .height(Length::Pct(100.0))
        .spaced(COLUMN_GAP)
        .style(
            Style::new()
                .flex_grow(1.0)
                .min_width(Length::Pct(0.0))
                .min_height(Length::Pct(0.0)),
        );
    if visible {
        let list_height =
            (viewport.h - editor_ui::chrome::TITLEBAR_HEIGHT - COLUMN_GAP.px()).max(0.0);
        let list = thread_list_view(sidebar, core, theme, list_height, visuals, emit.clone())
            .unwrap_or_else(|| UiBox::new().style(Style::new().flex_grow(1.0)).build());
        column = column.child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_height(Length::Pct(0.0)))
                .child(list)
                .child(sidebar.threads_edges.band(
                    "sidebar-threads-edge-top",
                    editor_ui::scroll_edge::Edge::Top,
                    theme.colors.background,
                ))
                .child(sidebar.threads_edges.band(
                    "sidebar-threads-edge-bottom",
                    editor_ui::scroll_edge::Edge::Bottom,
                    theme.colors.background,
                ))
                .build()
                .with_key("sidebar-threads-hover-surface"),
        );
        if let Some(host) = sidebar.host_data(core) {
            if host.is_live()
                && let Some(error) = &host.thread_page(false).error
            {
                column = column.child(error_text(&theme, error.clone()));
            }
            if host.project_icons_failed() {
                let retry = emit.clone();
                column = column.child(
                    Button::text(
                        "sidebar-metadata-retry",
                        theme,
                        ButtonTone::Neutral,
                        "Retry project icons",
                    )
                    .on_activate(move |_| retry(ViewAction::RetryProjectIcons))
                    .build(),
                );
            }
        }
        if let Some(error) = &sidebar.error {
            let dismiss = emit.clone();
            column = column.child(
                UiBox::new()
                    .layout("column")
                    .child(error_text(&theme, error.clone()))
                    .child(
                        Button::text(
                            "sidebar-error-dismiss",
                            theme,
                            ButtonTone::Quiet,
                            "Dismiss",
                        )
                        .on_activate(move |_| dismiss(ViewAction::DismissError))
                        .build(),
                    )
                    .build(),
            );
        }
    }
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(Length::Pct(100.0))
        .child(rail)
        .child(column.build().with_key("sidebar"))
        .build()
        .with_key(ROOT_KEY)
}

fn error_text(theme: &ArtisanTheme, error: String) -> Element {
    TextRole::Caption
        .text(theme, error)
        .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
        .build()
}
