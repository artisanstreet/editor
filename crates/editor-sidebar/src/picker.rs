//! Forge-owned directory browsing is identical for every paired host.
use artisan_api::{CreateProject, DirListing, Project, ProjectId};
use editor_core::ForgeClient;
use editor_ui::theme::{ActiveTheme as _, Opacity, RadiusStep, RadiusTokens};
use editor_ui::{
    AssetId,
    asset_seam::asset_glyph,
    text_editor::{TextEditor, TextEditorEvent},
};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, Window, div, px, uniform_list,
};

pub enum PickerEvent {
    Added(Project),
}
pub struct FolderPicker {
    client: ForgeClient,
    listing: Option<DirListing>,
    path: Entity<TextEditor>,
    hidden: bool,
    rows: Vec<usize>,
    loading: bool,
    creating: bool,
    #[cfg(test)]
    create_completion_gate: Option<(
        std::sync::Arc<std::sync::atomic::AtomicBool>,
        tokio::sync::oneshot::Receiver<bool>,
    )>,
    #[cfg(test)]
    create_requests: Vec<CreateProject>,
    #[cfg(test)]
    browse_requests: usize,
    request: Option<CreateProject>,
    error: Option<String>,
    browse: Task<()>,
    create: Task<()>,
    _submitted: Subscription,
}
impl EventEmitter<PickerEvent> for FolderPicker {}
impl FolderPicker {
    pub(crate) fn new(client: ForgeClient, cx: &mut Context<Self>) -> Self {
        let path = cx.new(|cx| TextEditor::new("", "Path on Forge", cx).single_line());
        let submitted = cx.subscribe(&path, |this, path, event: &TextEditorEvent, cx| {
            if *event == TextEditorEvent::Submitted {
                this.browse(Some(path.read(cx).text().to_owned()), cx);
            }
        });
        let mut picker = Self {
            client,
            listing: None,
            path,
            hidden: false,
            rows: Vec::new(),
            loading: false,
            creating: false,
            #[cfg(test)]
            create_completion_gate: None,
            #[cfg(test)]
            create_requests: Vec::new(),
            #[cfg(test)]
            browse_requests: 0,
            request: None,
            error: None,
            browse: Task::ready(()),
            create: Task::ready(()),
            _submitted: submitted,
        };
        picker.browse(None, cx);
        picker
    }
    pub(crate) const fn creating(&self) -> bool {
        self.creating
    }
    fn filter(&mut self) {
        self.rows = self
            .listing
            .as_ref()
            .map(|listing| {
                listing
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| {
                        entry.is_dir && (self.hidden || !entry.name.starts_with('.'))
                    })
                    .map(|(index, _)| index)
                    .collect()
            })
            .unwrap_or_default();
    }
    fn browse(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        if self.creating {
            return;
        }
        #[cfg(test)]
        {
            self.browse_requests += 1;
        }
        self.loading = true;
        self.error = None;
        let client = self.client.clone();
        self.browse = cx.spawn(async move |this, cx| {
            let result = client.list_dir(path.as_deref()).await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(listing) => {
                        this.path
                            .update(cx, |field, cx| field.set_text(&listing.path, cx));
                        this.listing = Some(listing);
                        this.request = None;
                        this.filter();
                    }
                    Err(error) => {
                        this.error = Some(format!("Couldn't open this folder: {error:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }
    fn add(&mut self, cx: &mut Context<Self>) {
        if self.creating || self.loading {
            return;
        }
        let Some(path) = self.listing.as_ref().map(|listing| listing.path.clone()) else {
            return;
        };
        let request = self
            .request
            .get_or_insert_with(|| CreateProject {
                id: ProjectId::new(),
                path,
                name: None,
            })
            .clone();
        self.creating = true;
        self.error = None;
        self.path.update(cx, |path, cx| path.set_disabled(true, cx));
        let client = self.client.clone();
        #[cfg(test)]
        let completion_gate = {
            self.create_requests.push(request.clone());
            self.create_completion_gate.take()
        };
        self.create = cx.spawn(async move |this, cx| {
            let result = client.create_project(&request).await;
            #[cfg(test)]
            let result = if let Some((arrived, release)) = completion_gate {
                arrived.store(true, std::sync::atomic::Ordering::Release);
                if release.await.unwrap_or(false) {
                    Err(anyhow::anyhow!("injected project acknowledgement failure"))
                } else {
                    result
                }
            } else {
                result
            };
            this.update(cx, |this, cx| {
                this.creating = false;
                this.path
                    .update(cx, |path, cx| path.set_disabled(false, cx));
                match result {
                    Ok(project) => cx.emit(PickerEvent::Added(project)),
                    Err(error) => this.error = Some(format!("Couldn't add this folder: {error:#}")),
                }
                cx.notify();
            })
            .ok();
        });
        cx.notify();
    }
}
impl Render for FolderPicker {
    #[expect(
        clippy::too_many_lines,
        reason = "the picker's root element tree, ported from the old Editor"
    )]
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = *cx.theme();
        let busy = self.loading || self.creating;
        let parent = self
            .listing
            .as_ref()
            .and_then(|listing| listing.parent.clone());
        let count = self.rows.len() + usize::from(parent.is_some());
        let list = uniform_list(
            "sidebar-folder-list",
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let (path, label, icon) = if index == 0 && parent.is_some() {
                            (parent.clone()?, "..".to_owned(), AssetId::TABLER_ARROW_UP)
                        } else {
                            let entry = this
                                .listing
                                .as_ref()?
                                .entries
                                .get(*this.rows.get(index - usize::from(parent.is_some()))?)?;
                            (
                                entry.path.clone(),
                                entry.name.clone(),
                                AssetId::TABLER_FOLDER,
                            )
                        };
                        let owner = cx.entity().downgrade();
                        Some(
                            div()
                                .id(SharedString::from(format!("folder-{path}")))
                                .role(gpui::Role::Button)
                                .aria_label(label.clone())
                                .when(!busy, |row| row.focusable().tab_index(0))
                                .h(px(30.0))
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .px(px(8.0))
                                .rounded(RadiusTokens::value(RadiusStep::Sm))
                                .when(!busy, |row| {
                                    row.cursor_pointer()
                                        .hover(move |style| style.bg(theme.colors.hover.to_paint()))
                                })
                                .child(
                                    asset_glyph(icon)
                                        .size(px(15.0))
                                        .text_color(theme.colors.text_muted.to_paint()),
                                )
                                .child(div().flex_1().min_w_0().truncate().child(label))
                                .on_click(move |_, _, cx| {
                                    if busy {
                                        return;
                                    }
                                    owner
                                        .update(cx, |this, cx| this.browse(Some(path.clone()), cx))
                                        .ok();
                                }),
                        )
                    })
                    .collect()
            }),
        )
        .h(px(320.0));
        let path = self
            .listing
            .as_ref()
            .map_or_else(|| "Loading…".to_owned(), |listing| listing.path.clone());
        let can_add = !busy && self.listing.is_some();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(10.0))
            .text_size(theme.typography.control_text)
            .text_color(theme.colors.text.to_paint())
            .child(
                div()
                    .truncate()
                    .text_color(theme.colors.text_muted.to_paint())
                    .child(path),
            )
            .child(list)
            .children(self.error.clone().map(|error| {
                div()
                    .text_color(theme.colors.destructive_text.to_paint())
                    .child(error)
            }))
            .child(
                div().flex().justify_end().child(
                    div()
                        .id("sidebar-folder-add")
                        .role(gpui::Role::Button)
                        .aria_label("Add this folder")
                        .debug_selector(|| "sidebar-folder-add".into())
                        .h(px(30.0))
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .rounded(RadiusTokens::value(RadiusStep::Md))
                        .bg(theme.colors.primary.to_paint())
                        .text_color(theme.colors.primary_text.to_paint())
                        .when(!can_add, |button| button.opacity(Opacity::Disabled.alpha()))
                        .when(can_add, |button| {
                            button
                                .focusable()
                                .tab_index(0)
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.add(cx)))
                        })
                        .child(if self.creating {
                            "Adding…"
                        } else {
                            "Add this folder"
                        }),
                ),
            )
    }
}

#[cfg(test)]
mod request_tests {
    use crate::tests::Setup;
    use gpui::{TestAppContext, VisualTestContext};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    #[gpui::test]
    fn create_freezes_browse_and_dismissal_and_selects_only_the_acknowledged_project(
        cx: &mut TestAppContext,
    ) {
        let setup = Setup::new(cx);
        setup.wait(cx, "paired host", |setup, cx| {
            setup.core.read_with(cx, |core, cx| {
                core.active_host()
                    .is_some_and(|host| host.read(cx).is_live())
            })
        });
        setup
            .window
            .update(cx, |sidebar, _, cx| sidebar.add_project(cx))
            .unwrap();
        let picker = setup
            .window
            .read_with(cx, |sidebar, _| sidebar.picker.as_ref().unwrap().0.clone())
            .unwrap();
        setup.wait(cx, "initial listing", |_, cx| {
            picker.read_with(cx, |picker, _| !picker.loading)
        });
        let path = setup.dir.path().join("new-project");
        std::fs::create_dir(&path).unwrap();
        picker.update(cx, |picker, cx| {
            picker.browse(Some(path.display().to_string()), cx);
        });
        setup.wait(cx, "project directory listing", |_, cx| {
            picker.read_with(cx, |picker, _| !picker.loading)
        });
        let arrived = Arc::new(AtomicBool::new(false));
        let (release, receive) = tokio::sync::oneshot::channel();
        picker.update(cx, |picker, cx| {
            picker.create_completion_gate = Some((Arc::clone(&arrived), receive));
            picker.add(cx);
        });
        setup.wait(cx, "server creation acknowledged but held", |_, _| {
            arrived.load(Ordering::Acquire)
        });
        let (request, browse_count) = picker.read_with(cx, |picker, _| {
            (picker.create_requests[0].clone(), picker.browse_requests)
        });
        assert!(
            setup
                .forge
                .block_on(setup.store.project(&request.id))
                .unwrap()
                .is_some()
        );
        picker.update(cx, |picker, cx| {
            picker.browse(Some("/must-not-replace-create".into()), cx);
            picker.add(cx);
        });
        assert_eq!(
            picker.read_with(cx, |picker, _| picker.browse_requests),
            browse_count
        );
        assert_eq!(
            picker.read_with(cx, |picker, _| picker.create_requests.len()),
            1
        );
        {
            let mut visual = VisualTestContext::from_window(setup.window.into(), cx);
            visual.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
            visual.simulate_keystrokes("escape");
            visual.run_until_parked();
        }
        assert!(
            setup
                .window
                .read_with(cx, |sidebar, _| sidebar.picker.is_some())
                .unwrap(),
            "creation acknowledgement must survive Escape"
        );
        release.send(false).unwrap();
        setup.wait(cx, "acknowledged project selected and dialog closed", |setup, cx| {
            setup.window.read_with(cx, |sidebar, _| sidebar.picker.is_none()).unwrap()
                && setup.core.read_with(cx, |core, _| matches!(core.screen(), editor_core::Screen::NewThread { project: Some(id), .. } if *id == request.id))
        });
        assert_eq!(
            picker.read_with(cx, |picker, _| picker.create_requests.len()),
            1
        );
    }

    #[gpui::test]
    fn lost_project_acknowledgement_retries_the_identical_create_identity(cx: &mut TestAppContext) {
        let setup = Setup::new(cx);
        setup.wait(cx, "paired host", |setup, cx| {
            setup.core.read_with(cx, |core, cx| {
                core.active_host()
                    .is_some_and(|host| host.read(cx).is_live())
            })
        });
        setup
            .window
            .update(cx, |sidebar, _, cx| sidebar.add_project(cx))
            .unwrap();
        let picker = setup
            .window
            .read_with(cx, |sidebar, _| sidebar.picker.as_ref().unwrap().0.clone())
            .unwrap();
        setup.wait(cx, "initial listing", |_, cx| {
            picker.read_with(cx, |picker, _| !picker.loading)
        });
        let path = setup.dir.path().join("retry-project");
        std::fs::create_dir(&path).unwrap();
        picker.update(cx, |picker, cx| {
            picker.browse(Some(path.display().to_string()), cx);
        });
        setup.wait(cx, "project directory listing", |_, cx| {
            picker.read_with(cx, |picker, _| !picker.loading)
        });
        let arrived = Arc::new(AtomicBool::new(false));
        let (release, receive) = tokio::sync::oneshot::channel();
        picker.update(cx, |picker, cx| {
            picker.create_completion_gate = Some((Arc::clone(&arrived), receive));
            picker.add(cx);
        });
        setup.wait(cx, "held accepted creation", |_, _| {
            arrived.load(Ordering::Acquire)
        });
        let id = picker.read_with(cx, |picker, _| picker.create_requests[0].id.clone());
        release.send(true).unwrap();
        setup.wait(cx, "reported acknowledgement failure", |_, cx| {
            picker.read_with(cx, |picker, _| !picker.creating && picker.error.is_some())
        });
        assert!(
            setup
                .window
                .read_with(cx, |sidebar, _| sidebar.picker.is_some())
                .unwrap()
        );
        assert_eq!(
            picker.read_with(cx, |picker, _| picker.request.as_ref().unwrap().id.clone()),
            id
        );
        picker.update(cx, super::FolderPicker::add);
        setup.wait(cx, "idempotent retry selected project", |setup, cx| setup.core.read_with(cx, |core, _| matches!(core.screen(), editor_core::Screen::NewThread { project: Some(project), .. } if *project == id)));
        assert_eq!(
            picker.read_with(cx, |picker, _| picker
                .create_requests
                .iter()
                .map(|request| request.id.clone())
                .collect::<Vec<_>>()),
            vec![id.clone(), id.clone()]
        );
        assert!(
            setup
                .forge
                .block_on(setup.store.project(&id))
                .unwrap()
                .is_some()
        );
    }
    #[gpui::test]
    #[expect(
        clippy::too_many_lines,
        reason = "the test sets up two Forge hosts and switches between them"
    )]
    fn creation_acknowledgement_on_old_host_cannot_navigate_the_new_host(cx: &mut TestAppContext) {
        let setup = Setup::new(cx);
        setup.wait(cx, "paired host", |setup, cx| {
            setup.core.read_with(cx, |core, cx| {
                core.active_host()
                    .is_some_and(|host| host.read(cx).is_live())
            })
        });
        setup
            .window
            .update(cx, |sidebar, _, cx| sidebar.add_project(cx))
            .unwrap();
        let picker = setup
            .window
            .read_with(cx, |sidebar, _| sidebar.picker.as_ref().unwrap().0.clone())
            .unwrap();
        setup.wait(cx, "initial listing", |_, cx| {
            picker.read_with(cx, |picker, _| !picker.loading)
        });
        let path = setup.dir.path().join("old-host-project");
        std::fs::create_dir(&path).unwrap();
        picker.update(cx, |picker, cx| {
            picker.browse(Some(path.display().to_string()), cx);
        });
        setup.wait(cx, "project directory", |_, cx| {
            picker.read_with(cx, |picker, _| !picker.loading)
        });
        let arrived = Arc::new(AtomicBool::new(false));
        let (release, receive) = tokio::sync::oneshot::channel();
        picker.update(cx, |picker, cx| {
            picker.create_completion_gate = Some((Arc::clone(&arrived), receive));
            picker.add(cx);
        });
        setup.wait(cx, "held creation acknowledgement", |_, _| {
            arrived.load(Ordering::Acquire)
        });
        let created = picker.read_with(cx, |picker, _| picker.create_requests[0].id.clone());
        let server = setup
            .forge
            .block_on(forge_http::start(forge_http::ServeConfig {
                data_dir: setup.dir.path().join("second-forge"),
                listen: "127.0.0.1:0".parse().unwrap(),
            }))
            .unwrap();
        let address = server.local_addr().unwrap().to_string();
        let store = server.store().clone();
        setup.forge.spawn(server.run());
        let (_, host) = crate::tests::forge_fixture::pair(
            &store,
            &setup.forge,
            &address,
            &setup.dir.path().join("editor"),
        );
        let baseline = editor_core::Screen::NewThread {
            host: host.id.clone(),
            project: None,
        };
        setup.core.update(cx, |core, cx| {
            core.add_host(host.clone(), cx).unwrap();
            core.navigate(baseline.clone(), cx);
        });
        setup.wait(cx, "new host selected", |setup, cx| {
            setup
                .window
                .read_with(cx, |sidebar, cx| {
                    sidebar
                        .host
                        .as_ref()
                        .is_some_and(|data| data.read(cx).id == host.id)
                })
                .unwrap()
        });
        setup
            .window
            .update(cx, |sidebar, _, _| {
                sidebar.error = Some("new host error".into());
            })
            .unwrap();
        release.send(false).unwrap();
        setup.wait(cx, "old picker acknowledgement releases", |setup, cx| {
            setup
                .window
                .read_with(cx, |sidebar, _| sidebar.picker.is_none())
                .unwrap()
        });
        assert_eq!(
            setup.core.read_with(cx, |core, _| core.screen().clone()),
            baseline
        );
        assert_eq!(
            setup
                .window
                .read_with(cx, |sidebar, _| sidebar.error.clone())
                .unwrap(),
            Some("new host error".into())
        );
        assert!(
            setup
                .forge
                .block_on(setup.store.project(&created))
                .unwrap()
                .is_some()
        );
        assert!(
            setup
                .forge
                .block_on(store.project(&created))
                .unwrap()
                .is_none()
        );
    }
}
