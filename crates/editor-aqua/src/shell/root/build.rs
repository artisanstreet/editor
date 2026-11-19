use super::{ResizeKey, RootViews, overview_shown};
use crate::core::Screen;
use crate::shell::{application::BuildError, composer_glide::Route, host::Shell, layout, view};
use aqua::prelude::{AquaWindowId, ObservedWindowState};
use aqua::{AppCtx, UiApp};
use aqua_ui::element::Element;
use std::rc::Rc;

impl RootViews {
    #[expect(
        clippy::too_many_lines,
        reason = "one root build that assembles every screen region from the same frame state"
    )]
    pub(super) fn build_root(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> Result<Element, BuildError> {
        let theme = shell.preferences.theme();
        let account = self.account(shell);
        if self.image_preview_stale(shell) {
            self.close_image_preview(ui, false);
        }
        if self.color_scope.as_ref().is_some_and(|scope| {
            !shell
                .entry
                .workspace()
                .is_some_and(|workspace| scope.current(&workspace.conversation, &shell.core))
        }) {
            self.dispose_color_menu(ui);
        }
        let viewport = layout::Size {
            width: crate::measure::narrow(window.inner_size.width),
            height: crate::measure::narrow(window.inner_size.height),
        };
        if let Some(entry) = shell
            .entry
            .view(&shell.core, &theme, &shell.titlebar, window)
        {
            self.release_workspace_views(ui);
            self.connection.reset();
            self.thread_loading.reset();
            self.thread_landing.reset();
            self.link_failure.close(ui);
            self.workspace_key = None;
            self.settings_panel = None;
            return Ok(entry);
        }
        let app_media = Self::app_media(shell, window);
        let workspace = shell.entry.workspace_mut().ok_or(BuildError::NoWorkspace)?;
        if self.workspace_key != Some(workspace.selections.key) {
            self.release_workspace_views(ui);
            self.workspace_key = Some(workspace.selections.key);
        }
        let screen = match shell.core.workspace_screen() {
            Screen::Settings(_) => Screen::Home,
            screen @ (Screen::Home
            | Screen::Thread { .. }
            | Screen::NewThread { .. }
            | Screen::Marketplace
            | Screen::Usage) => screen.clone(),
        };
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let size = aqua_core::Size::new(viewport.width, viewport.height);
        let now = shell.core.clock().now();
        self.sidebar.fit(viewport.width, now, reduced);
        aqua_core::checkpoint::reach();
        let card_width =
            (viewport.width - self.sidebar.width(now) - crate::frame::view::SHEET_PADDING).max(0.0);
        self.resize_key = Some(ResizeKey::of(&screen, card_width, viewport));
        let lift = crate::conversation::view::composer_lift(card_width);
        let sent = workspace.take_sent_new_thread();
        let route = match &screen {
            Screen::NewThread { .. } => Route::NewThread(workspace.new_thread_posting()),
            Screen::Thread { thread, .. } => Route::Thread { thread, sent },
            Screen::Home | Screen::Settings(_) | Screen::Marketplace | Screen::Usage => {
                Route::Elsewhere
            }
        };
        self.composer_glide.follow(route, lift, now, reduced);
        let sky = view::Sky {
            opacity: self.composer_glide.sky_opacity(now),
            transform: self.composer_glide.sky_transform(now),
        };
        let mut connection_held = false;
        let center = match &screen {
            Screen::Home => workspace.home_view(&shell.core, &theme),
            Screen::Marketplace => workspace.marketplace_view(&shell.core, &theme),
            Screen::Usage => workspace.usage_view(&shell.core, &theme),
            Screen::Thread { .. } => {
                workspace.conversation.sync_screen(&mut shell.core);
                if reduced {
                    workspace.conversation.skip_reveal();
                }
                if workspace.conversation.host_is_current(&shell.core) {
                    self.thread_content(
                        workspace,
                        &shell.core,
                        ui,
                        &theme,
                        app_media,
                        super::thread_content::ContentPass {
                            width: card_width,
                            reduced_motion: reduced,
                            swift: shell.preferences.streaming
                                == crate::ui::preferences::StreamingMode::Swift,
                            now,
                            transcript_only: false,
                        },
                    )?
                } else {
                    self.replace_rows(None);
                    if let Screen::Thread { host, thread } = &screen {
                        self.connection
                            .hold(Some(crate::shell::connection::Owner::Thread(
                                host.clone(),
                                thread.clone(),
                            )));
                    }
                    connection_held = true;
                    view::centered_conversation(self.connection.waiting(&theme, now, reduced))
                }
            }
            Screen::NewThread { host, .. } => {
                self.connection
                    .hold(Some(crate::shell::connection::Owner::NewThread(
                        host.clone(),
                    )));
                connection_held = true;
                let host = shell
                    .core
                    .host(host)
                    .ok_or_else(|| BuildError::HostGone { host: host.clone() })?;
                if let Some(status) = self.connection.build(host, &theme, now, reduced) {
                    view::centered_conversation(status)
                } else {
                    let (questionnaires, attachments) =
                        workspace.composer_content(ui, &theme, reduced)?;
                    view::new_conversation(
                        workspace,
                        &shell.core,
                        &theme,
                        view::ComposerParts {
                            questionnaires,
                            attachments,
                            mention_overlay: None,
                            thread_error: None,
                            shift: self.composer_glide.shift(now),
                            measurement: Rc::clone(&self.composer_measurement),
                            above: None,
                        },
                        reduced,
                        self.composer_glide.launched().then_some(card_width),
                        self.composer_glide.sky_shown(now).then_some(&sky),
                    )?
                }
            }
            Screen::Settings(_) => unreachable!(),
        };
        let center = match &screen {
            Screen::Thread { .. } => view::under_setting_sky(
                &theme,
                Rc::clone(&self.composer_measurement),
                reduced,
                self.composer_glide.sky_setting(now).then_some(&sky),
                center,
            ),
            Screen::Home
            | Screen::NewThread { .. }
            | Screen::Settings(_)
            | Screen::Marketplace
            | Screen::Usage => center,
        };
        let connection_lost = match &screen {
            Screen::Thread { host, .. } => shell
                .core
                .host(host)
                .and_then(|host| crate::shell::connection::lost(&host.status)),
            Screen::Home
            | Screen::NewThread { .. }
            | Screen::Settings(_)
            | Screen::Marketplace
            | Screen::Usage => None,
        };
        aqua_core::checkpoint::reach();
        if !matches!(screen, Screen::Thread { .. }) {
            self.replace_rows(None);
            self.stored.borrow_mut().drain_cleanup(ui);
            self.observations.clear();
            self.navigation.dispose();
            self.transcript_viewport = None;
            self.mounted_reveals.borrow_mut().rebind(None);
            self.row_cache.borrow_mut().rebind(None);
            workspace.markdown.borrow_mut().rebind(ui, None);
            workspace.conversation_selection.rebind(ui, None);
            drop(self.media.take());
            self.thread_loading.reset();
            self.thread_landing.reset();
            self.smooth_scroll.cancel();
            self.scroll_request = None;
        }
        if !connection_held {
            self.connection.hold(None);
        }
        if let Some(palette) = &workspace.palette {
            if self.palette.is_none() {
                self.palette = Some(crate::shell::palette::view::PaletteView::new(
                    self.palette_emit(),
                ));
            }
            self.palette_content = self
                .palette
                .as_ref()
                .map(|view| view.content(palette, &theme, self.palette_emit()));
        } else {
            self.close_palette(ui);
        }
        if let Some(preview) = &workspace.resource_preview {
            let generation = workspace.resource_preview_generation();
            if self.resource.as_ref().map(|(current, _)| *current) != Some(generation) {
                self.close_resource(ui);
                self.resource = Some((
                    generation,
                    crate::shell::resource_preview::view::ViewState::new(
                        generation,
                        workspace.selections.next_document(),
                        workspace.selections.next_source(),
                    ),
                ));
            }
            let emit = self.resource_emit();
            if let Some((_, state)) = &mut self.resource {
                state
                    .prepare(preview, ui)
                    .map_err(|source| BuildError::Selection {
                        surface: "resource preview",
                        source,
                    })?;
                let content = state
                    .content(preview, &theme, viewport.height, &emit)
                    .map_err(BuildError::ResourcePreview)?;
                self.resource_content = Some((content, preview.path.clone()));
            }
        } else {
            self.close_resource(ui);
        }
        let settings_open = matches!(shell.core.screen(), Screen::Settings(_));
        self.settings_panel = settings_open.then(|| {
            let settings_layout = layout::settings_layout(viewport);
            let nav = workspace.settings_sidebar_view(
                &shell.core,
                &theme,
                aqua_core::Size::new(settings_layout.size.width, settings_layout.size.height),
                reduced,
                now,
            );
            let page = workspace.settings_view(&shell.core, &theme, &shell.build, size, true);
            let dismissed = Rc::clone(&self.settings_dismissed);
            let dismiss: Rc<dyn Fn()> = Rc::new(move || dismissed.set(true));
            let panel = view::settings_panel(&theme, viewport, nav, page, &dismiss);
            (panel, settings_layout.size.width)
        });
        aqua_core::checkpoint::reach();
        let titlebar_scope = matches!(shell.core.screen(), Screen::Thread { .. })
            .then(|| {
                crate::conversation::presentation::PresentationScope::capture(
                    &workspace.conversation,
                    &shell.core,
                )
            })
            .flatten();
        self.context_reading = titlebar_scope.and_then(|scope| {
            let host = shell.core.host_if_instance(&scope.host, scope.instance)?;
            let subject =
                scope
                    .agent
                    .as_ref()
                    .map_or(crate::shell::context_meter::Subject::Thread, |id| {
                        crate::shell::context_meter::Subject::Agent(
                            workspace
                                .conversation
                                .view
                                .as_ref()
                                .and_then(|view| view.item(id))
                                .and_then(|item| {
                                    if let artisan_api::ItemKind::Agent {
                                        context_used: Some(context_used),
                                        model,
                                        ..
                                    } = &item.kind
                                    {
                                        Some((*context_used, model.clone()))
                                    } else {
                                        None
                                    }
                                }),
                        )
                    });
            crate::shell::context_meter::reading(host, &scope.thread, subject)
        });
        workspace
            .inspector
            .follow(&shell.core, &workspace.conversation, ui);
        let outline = self.navigation.chevron(std::time::Instant::now());
        let navigation = workspace
            .sidebar
            .host
            .as_ref()
            .and_then(|id| shell.core.host(id))
            .map(|host| {
                let open = workspace.sidebar.open.clone();
                let tabs = open
                    .as_ref()
                    .and_then(|id| {
                        let thread = host.threads.get(id)?;
                        let project = host.projects.get(&thread.project_id)?;
                        Some(crate::frame::strip::Tab {
                            id: id.clone(),
                            project: project.id.clone(),
                            trail: workspace
                                .conversation
                                .open
                                .as_ref()
                                .filter(|(shown, thread)| shown == &host.id && thread == id)
                                .map(|_| workspace.conversation.agent_trail())
                                .unwrap_or_default(),
                            title: crate::frame::projects::title_of(thread.title.as_deref()),
                            location: (!project.id.is_scratch()).then(|| {
                                host.repository(&project.id)
                                    .and_then(|repository| repository.web_url.as_deref())
                                    .map_or_else(
                                        || project.name.clone(),
                                        crate::sidebar::list::qualified_label,
                                    )
                            }),
                            url: host
                                .repository(&project.id)
                                .and_then(|repository| repository.web_url.clone()),
                            mark: crate::ui::project_mark::Mark::of(host, &project.id),
                            state: crate::sidebar::list::row_state(host, thread),
                            active: true,
                            outline,
                        })
                    })
                    .into_iter()
                    .collect::<Vec<_>>();
                let height =
                    crate::shell::wheel::viewport_height(ui, crate::sidebar::view::ROOT_KEY)
                        .unwrap_or_else(|| (viewport.height - 260.0).max(0.0));
                let tree = crate::frame::inbox::view(
                    &workspace.sidebar,
                    &shell.core,
                    theme,
                    height,
                    reduced,
                    &self.tree_emit(workspace),
                );
                let strip =
                    crate::frame::strip::strip(&theme, &tabs, reduced, &self.sidebar.emit());
                (tree, strip)
            });
        let (tree, strip) = match navigation {
            Some((tree, strip)) => (Some(tree), strip),
            None => (None, Vec::new()),
        };
        let (search, tree, overlay) = self.search_parts(workspace, &theme, tree, now);
        let overview = match (overview_shown(&screen, card_width), workspace.side_panel) {
            (false, _) => None,
            (true, crate::shell::workspace::SidePanel::Overview) => crate::frame::overview::card(
                &theme,
                &crate::frame::overview::Overview::of(
                    &shell.core,
                    workspace.inspector.inspector(),
                    self.context_reading.clone(),
                ),
                reduced,
                &workspace.inspector_emit(),
            ),
            (true, crate::shell::workspace::SidePanel::Inspector) => {
                let panel = workspace
                    .inspector_panel(
                        &shell.core,
                        ui,
                        &theme,
                        crate::frame::overview::panel_height(viewport.height),
                        reduced,
                        self.ui_now,
                    )
                    .map_err(BuildError::Inspector)?;
                Some(crate::frame::overview::panel(&theme, panel))
            }
        };
        aqua_core::checkpoint::reach();
        let titlebar = shell.titlebar.workspace(&theme, window, strip, None, None);
        let image_preview = self.image_preview.as_ref().map(|preview| {
            let events = Rc::clone(&self.preview_actions);
            preview.view.view(
                theme,
                aqua_core::Size::new(viewport.width, viewport.height),
                now,
                reduced,
                Rc::new(move |generation, action| {
                    events.borrow_mut().push_back((generation, action));
                }),
            )
        });
        let body = crate::frame::view::body(
            &theme,
            titlebar,
            self.sidebar_model(&theme, search, tree, account.as_ref(), now),
            overlay,
            vec![
                aqua::Aqua::Box()
                    .layout("row")
                    .styles([aqua::Aqua::Style()
                        .flex_grow(1.0)
                        .min_width(aqua::pct(0.0))
                        .min_height(aqua::pct(0.0))])
                    .child(
                        aqua::Aqua::Box()
                            .layout("column")
                            .styles([aqua::Aqua::Style().flex_grow(1.0).min_width(aqua::pct(0.0))])
                            .child(center)
                            .children(connection_lost)
                            .build(),
                    )
                    .children(overview)
                    .build(),
            ],
            Rc::clone(&self.seam),
            reduced,
        );
        Ok(view::workspace(
            workspace.selections.key,
            &theme,
            view::WorkspaceElements {
                body,
                settings_dialog: None,
                palette: None,
                resource_preview: None,
                copy_menu: None,
                image_preview,
            },
        ))
    }

    pub(super) fn after_mount(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        self.sync_account_menu(shell, ui, ctx, window);
        self.sync_sign_in(shell, ui, ctx, window);
        if self.sidebar.take_search_focus() {
            ui.focus_key(crate::frame::search::QUERY);
        }
        self.source_dialogs
            .retain(|export| Self::source_export_is_current(shell, export));
        self.bind_row_sources(ui, ctx, window);
        (|| {
            let Some((panel, width)) = self.settings_panel.take() else {
                if let Some(dialog) = self.settings.take() {
                    dialog.dispose(ui);
                }
                return;
            };
            if let Some(dialog) = &self.settings {
                if dialog.is_open(ui) {
                    dialog.set_theme(shell.preferences.theme());
                    if let Err(error) = dialog.refresh_content(ui, panel) {
                        tracing::warn!(%error, "could not refresh settings dialog");
                    }
                    return;
                }
                self.settings = None;
            }
            let Some(owner) = ui.find_debug_selector("workspace-root") else {
                return;
            };
            let dismissed = Rc::clone(&self.settings_dismissed);
            let dialog = crate::ui::dialog::Dialog::new(
                "settings-dialog",
                shell.preferences.theme(),
                "Settings",
                panel,
            )
            .bare()
            .soft_backdrop()
            .max_width(width)
            .on_dismiss(move |_| dismissed.set(true));
            match crate::ui::dialog::DialogHost::open(ui, owner, dialog) {
                Ok(dialog) => self.settings = Some(dialog),
                Err(error) => tracing::warn!(%error, "could not open settings dialog"),
            }
        })();
        self.mount_palette(shell, ui);
        self.mount_resource(shell, ui);
        self.link_failure.mount(ui, &shell.preferences.theme());
        if self.sync_image_preview(shell, ui, ctx, window) {
            crate::core::diagnostics::rebuild_stats::record_event("inv_preview");
            self.conversation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
        self.sync_resource_jump(ui, ctx, window);
        {
            let _tm = crate::shell::application::Tm::new("input.views.context");
            self.sync_context(shell, ui, ctx, window);
        }
        let navigation_changed = {
            let _tm = crate::shell::application::Tm::new("input.views.navigation");
            self.sync_navigation(ui, ctx, window)
        };
        if navigation_changed {
            crate::core::diagnostics::rebuild_stats::record_event("inv_nav_fc");
            self.navigation_invalidated.set(true);
        }
        if self.sync_sidebar(shell, ui, ctx, window) {
            crate::core::diagnostics::rebuild_stats::record_event("inv_sidebar_a");
            self.conversation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
        if self
            .resource
            .as_ref()
            .is_some_and(|(_, state)| state.pending_jump().is_some())
        {
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
    }
}
