//! Adding a project on a host with no system folder dialog of ours (a
//! remote Forge): browse its folders, then create the project there.

use artisan_api::{CreateProject, DirListing, Project, ProjectId};
use editor_core::ForgeClient;
use editor_ui::AssetId;
use editor_ui::asset_seam::asset_glyph;
use editor_ui::theme::ArtisanTheme;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Context, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, px,
};

pub(crate) enum PickerEvent {
    Added(Project),
}

pub(crate) struct FolderPicker {
    client: ForgeClient,
    theme: ArtisanTheme,
    listing: Option<DirListing>,
    busy: bool,
    error: Option<String>,
    _task: Task<()>,
}

impl EventEmitter<PickerEvent> for FolderPicker {}

impl FolderPicker {
    pub(crate) fn new(client: ForgeClient, theme: ArtisanTheme, cx: &mut Context<Self>) -> Self {
        let mut picker = Self {
            client,
            theme,
            listing: None,
            busy: false,
            error: None,
            _task: Task::ready(()),
        };
        picker.browse(None, cx);
        picker
    }

    /// Lists `path` on the Forge host; `None` is its home folder.
    fn browse(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let client = self.client.clone();
        self.busy = true;
        self.error = None;
        self._task = cx.spawn(async move |picker, cx| {
            let listing = client.list_dir(path.as_deref()).await;
            picker
                .update(cx, |picker, cx| {
                    picker.busy = false;
                    match listing {
                        Ok(listing) => picker.listing = Some(listing),
                        Err(error) => {
                            let place = path.as_deref().unwrap_or("the home folder");
                            tracing::warn!(path = place, "couldn't list folder: {error:#}");
                            picker.error = Some(format!("Couldn't open {place}: {error:#}"));
                        }
                    }
                    cx.notify();
                })
                .ok();
        });
        cx.notify();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.listing.as_ref().map(|listing| listing.path.clone()) else {
            return;
        };
        let client = self.client.clone();
        self.busy = true;
        self.error = None;
        self._task = cx.spawn(async move |picker, cx| {
            let created = client
                .create_project(&CreateProject {
                    id: ProjectId::new(),
                    path: path.clone(),
                    name: None,
                })
                .await;
            picker
                .update(cx, |picker, cx| {
                    picker.busy = false;
                    match created {
                        Ok(project) => cx.emit(PickerEvent::Added(project)),
                        Err(error) => {
                            tracing::warn!(path, "couldn't add project: {error:#}");
                            picker.error = Some(format!("Couldn't add {path}: {error:#}"));
                        }
                    }
                    cx.notify();
                })
                .ok();
        });
        cx.notify();
    }
}

/// Joins in the host's own separator: a Windows Forge lists `C:\…` paths.
fn join(parent: &str, name: &str) -> String {
    let separator = if parent.contains('\\') && !parent.contains('/') {
        '\\'
    } else {
        '/'
    };
    format!("{}{separator}{name}", parent.trim_end_matches(separator))
}

impl Render for FolderPicker {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let colors = &theme.colors;
        let muted = colors.muted_foreground.to_paint();
        let hover = colors.muted.to_paint();
        let row = |id: SharedString, icon: AssetId, label: SharedString| {
            div()
                .id(id)
                .h(px(30.0))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(8.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child(asset_glyph(icon).size(px(15.0)).text_color(muted))
                .child(div().flex_1().min_w(px(0.0)).truncate().child(label))
        };
        let mut list = div()
            .id("sidebar-folder-list")
            .h(px(320.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(1.0));
        if let Some(listing) = &self.listing {
            if let Some(parent) = listing.parent.clone() {
                list = list.child(
                    row("folder-up".into(), AssetId::TABLER_ARROW_UP, "..".into()).on_click(
                        cx.listener(move |picker, _, _, cx| {
                            picker.browse(Some(parent.clone()), cx)
                        }),
                    ),
                );
            }
            for entry in listing
                .entries
                .iter()
                .filter(|entry| entry.is_dir && !entry.name.starts_with('.'))
            {
                let path = join(&listing.path, &entry.name);
                list = list.child(
                    row(
                        SharedString::from(format!("folder-{}", entry.name)),
                        AssetId::TABLER_FOLDER,
                        entry.name.clone().into(),
                    )
                    .on_click(
                        cx.listener(move |picker, _, _, cx| picker.browse(Some(path.clone()), cx)),
                    ),
                );
            }
        }
        let path: SharedString = self
            .listing
            .as_ref()
            .map_or_else(|| "Loading…".into(), |listing| listing.path.clone().into());
        let can_add = self.listing.is_some() && !self.busy;
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .text_size(theme.typography.control_text)
            .text_color(colors.foreground.to_paint())
            .child(div().truncate().text_color(muted).child(path))
            .child(list)
            .when_some(self.error.clone(), |picker, error| {
                picker.child(div().text_color(colors.destructive.to_paint()).child(error))
            })
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("sidebar-folder-add")
                        .debug_selector(|| "sidebar-folder-add".to_owned())
                        .h(px(30.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .rounded(px(8.0))
                        .bg(colors.primary.to_paint())
                        .text_color(colors.primary_foreground.to_paint())
                        .when(!can_add, |button| button.opacity(0.5))
                        .when(can_add, |button| {
                            button
                                .cursor_pointer()
                                .on_click(cx.listener(|picker, _, _, cx| picker.add(cx)))
                        })
                        .child("Add this folder"),
                ),
            )
    }
}
