//! Home, titlebar, sidebar, and desktop route surface composition for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-3 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    /// Renders the home headline surface: a muted emblem, the centered
    /// "What should we buildâ€¦?" heading with the inline project switcher,
    /// and nothing else â€” the editable composer lives in the shell footer.
    /// The Failure branch keeps the actionable offline error; every other
    /// branch drops the old "Start a task" copy and subtitle.
    pub(super) fn new_thread_surface_section(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let root = div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .px(px(24.0))
            .pb(px(24.0))
            .debug_selector(|| DESKTOP_HOME_SELECTOR.to_string())
            .child(self.home_emblem());
        if self.connection_retry_pending {
            return root.child(self.home_heading("Reconnecting to Forge…"));
        }
        if let NativeViewState::Failure(failure) = &self.state {
            let (heading, detail) = match failure.category {
                ServiceFailureCategory::ConnectionBusy => (
                    "Host is already connected",
                    "Another editor window is using this host. Close that connection, then retry here.",
                ),
                ServiceFailureCategory::Authentication => (
                    "Could not authenticate with Forge",
                    "The saved connection credential is no longer usable. Restart Forge on the host, then retry here, or add a fresh host invitation.",
                ),
                _ => (
                    "Forge is offline",
                    "The editor could not connect to Forge. Existing project and task data will remain available when it reconnects.",
                ),
            };
            return root
                .child(self.home_heading(heading))
                .child(
                    div()
                        .mt(px(8.0))
                        .max_w(px(440.0))
                        .text_size(px(15.0))
                        .text_color(self.desktop_theme.secondary)
                        .child(detail)
                        .child(
                            div()
                                .mt(px(8.0))
                                .text_size(px(12.0))
                                .child(format!("{} · {}", failure.stage, failure.category)),
                        ),
                )
                .child(
                    div()
                        .id("retry-forge-connection")
                        .debug_selector(|| "retry-forge-connection".into())
                        .cursor_pointer()
                        .mt(px(16.0))
                        .px(px(16.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(self.desktop_theme.line)
                        .child("Retry connection")
                        .on_click(cx.listener(|app, _, _, cx| {
                            cx.emit(super::impl_machines::SelectMachine(
                                app.machine_home.clone(),
                            ));
                        })),
                );
        }
        match self.selected_project_name() {
            Some(name) => root.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .child(self.home_heading("What should we build in\u{a0}"))
                    .child(self.home_project_trigger(&name, window, cx))
                    .child(self.home_heading("?")),
            ),
            None => root
                .child(self.home_heading("What should we build?"))
                .child(self.home_project_trigger(HOME_CHOOSE_PROJECT_LABEL, window, cx)),
        }
    }

    /// Centered large regular home heading line.
    pub(super) fn home_heading(&self, text: &str) -> Div {
        div()
            .text_size(px(HOME_HEADLINE_TEXT_PX))
            .font_weight(FontWeight::NORMAL)
            .text_color(self.desktop_theme.foreground)
            .child(text.to_owned())
    }

    /// Small muted emblem above the home heading.
    pub(super) fn home_emblem(&self) -> Div {
        div().mb(px(16.0)).child(
            asset_glyph(AssetId::TABLER_TERMINAL_2)
                .size(px(HOME_EMBLEM_SIZE_PX))
                .text_color(self.desktop_theme.secondary),
        )
    }

    /// The inline project switcher for the home heading: the live picker
    /// when installed, otherwise the static label (before the first project
    /// listing arrives). Text styling inherits from the heading container.
    pub(super) fn home_project_trigger(
        &mut self,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(home) = self.home_picker.clone() else {
            return div()
                .text_size(px(HOME_HEADLINE_TEXT_PX))
                .font_weight(FontWeight::NORMAL)
                .text_color(self.desktop_theme.foreground)
                .child(label.to_owned())
                .into_any_element();
        };
        home.update(cx, |picker, picker_cx| {
            picker.render_inline_trigger(label, window, picker_cx)
        })
    }

    pub(super) fn selected_project_name(&self) -> Option<String> {
        self.selected_project.as_ref().and_then(|selected| {
            self.project_options
                .iter()
                .find(|option| &option.id == selected)
                .map(|option| option.name.to_string())
        })
    }

    /// Requests the selected project's repository facts when the selection
    /// changes, clearing stale facts first.
    ///
    /// The read is a decoration, not a gate: a submission failure keeps the
    /// project-folder fallback instead of failing the workspace, and the fence
    /// resets so a later selection can retry.
    pub(super) fn request_project_repository(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_project.clone();
        if self.titlebar_repository_project == selected {
            return;
        }
        self.titlebar_repository_project.clone_from(&selected);
        self.titlebar_repository = None;
        let Some(project_id) = selected else {
            cx.notify();
            return;
        };
        if self
            .submit_command(NativeTransportCommand::QueryProjectRepository { project_id })
            .is_err()
        {
            self.titlebar_repository_project = None;
        }
        cx.notify();
    }

    /// Retains one inspected repository observation for the titlebar header.
    ///
    /// A reply for a project that is no longer selected is dropped: the
    /// selection fence, not arrival order, decides what the header shows.
    pub(super) fn handle_project_repository(
        &mut self,
        project_id: &ProjectId,
        repository: Option<&artisan_protocol::ProjectRepository>,
        cx: &mut Context<Self>,
    ) {
        if self.selected_project.as_ref() != Some(project_id) {
            return;
        }
        self.titlebar_repository = repository.and_then(titlebar_repository_for_project);
        cx.notify();
    }

    /// Clears retained repository facts after a failed read for the selection.
    pub(super) fn handle_project_repository_failure(
        &mut self,
        project_id: &ProjectId,
        cx: &mut Context<Self>,
    ) {
        if self.selected_project.as_ref() == Some(project_id) {
            self.titlebar_repository = None;
            cx.notify();
        }
    }

    /// Native titlebar sidebar section: the `Artisan Editor` wordmark, which
    /// keeps the home navigation.
    ///
    /// The wordmark stays in the leading section above the sidebar; the
    /// workspace header naming the open project and conversation lives in the
    /// titlebar's content section, anchored to the primary card's left edge.
    pub(super) fn desktop_brand(&self, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .child(
                div()
                    .id("artisan-brand-home")
                    .block_mouse_except_scroll()
                    .line_height(px(22.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.navigate(NativeRoute::NewThread { project: None }, cx);
                    }))
                    .debug_selector(|| "artisan-brand-home".to_owned())
                    .flex_shrink_0()
                    .text_size(px(20.0))
                    .font_family("Artisan Neo")
                    .font_weight(FontWeight::SEMIBOLD)
                    // -0.05em tracking at 20px: 20 * -0.05 = -1.0px.
                    .letter_spacing(px(-1.0))
                    .text_color(self.desktop_theme.foreground)
                    .child("Artisan Editor"),
            )
    }

    /// The titlebar workspace header, when the route names a conversation.
    ///
    /// The line follows the reference strip: a repository host mark and its
    /// qualified `owner/repository` link when inspected repository facts
    /// exist, the project folder otherwise, then the darker `/` separator and
    /// the conversation title. A route that names no conversation returns
    /// `None`, leaving the bare wordmark.
    pub(super) fn desktop_header_cluster(&self, cx: &App) -> Option<AnyElement> {
        let thread_title = self.desktop_header_title(cx)?;
        let project_display_name = self.selected_project_name();
        let presentation = present_titlebar_header(TitlebarHeaderInput::new(
            project_display_name.as_deref(),
            self.titlebar_repository.as_ref(),
            Some(thread_title.as_str()),
        ))?;
        let mut cluster = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .min_w(px(0.0))
            .overflow_hidden()
            // The reference line paints the workspace context in muted
            // foreground; only the repository link, the host mark, and the
            // darker separator override it.
            .text_color(titlebar_context_tone(&self.theme))
            .debug_selector(|| TITLEBAR_HEADER_SELECTOR.to_owned());
        for segment in presentation.segments() {
            cluster = cluster.child(self.render_titlebar_segment(segment));
        }
        Some(cluster.into_any_element())
    }

    /// Renders one titlebar workspace-header segment.
    ///
    /// The cluster carries the muted reference line, so only the repository
    /// link, the separator, and monochrome marks override the inherited
    /// color; the thread title is the elastic, truncating segment.
    pub(super) fn render_titlebar_segment(
        &self,
        segment: &TitlebarHeaderSegment<'_>,
    ) -> AnyElement {
        let theme = self.theme;
        match segment {
            TitlebarHeaderSegment::RepositoryMark { host } => {
                let mark = repository_mark_for(Some(*host));
                let mut glyph = asset_glyph(repository_logo_asset(mark.logo)).size(px(14.0));
                if mark.monochrome {
                    // The reference inverts single-color marks with the
                    // theme; the native tint does the same on the dark shell.
                    glyph = glyph.text_color(theme.colors.foreground.to_paint());
                }
                div()
                    .flex_shrink_0()
                    .debug_selector(|| TITLEBAR_REPOSITORY_MARK_SELECTOR.to_owned())
                    .child(glyph)
                    .into_any_element()
            }
            TitlebarHeaderSegment::RepositoryLink { label, web_url } => {
                let destination = SharedString::from((*web_url).to_owned());
                let selector = format!("{TITLEBAR_REPOSITORY_LABEL_SELECTOR}:{label}");
                div()
                    .id("artisan-desktop-titlebar-repository-link")
                    .cursor_pointer()
                    // The reference keeps the repository label at its natural
                    // width: only the thread subject ellipsizes.
                    .flex_shrink_0()
                    .text_color(theme.colors.banner_info.to_paint())
                    .debug_selector(move || selector.clone())
                    .on_click(move |_, _, cx| cx.open_url(destination.as_ref()))
                    .child(SharedString::from(label.clone()))
                    .into_any_element()
            }
            TitlebarHeaderSegment::ProjectFolder { label } => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .flex_shrink_0()
                // The folder fallback is workspace context: icon and label
                // both carry the muted context tone, never the foreground.
                .text_color(titlebar_context_tone(&self.theme))
                .debug_selector(|| TITLEBAR_PROJECT_FOLDER_SELECTOR.to_owned())
                .child(
                    asset_glyph(AssetId::TABLER_FOLDER)
                        .size(px(14.0))
                        .text_color(titlebar_context_tone(&self.theme))
                        .flex_shrink_0(),
                )
                .child(SharedString::from((*label).to_owned()))
                .into_any_element(),
            TitlebarHeaderSegment::ThreadSeparator => div()
                .flex_shrink_0()
                // The reference paints the separator even darker than the
                // muted line around it; S600 is two ramp steps below the
                // muted-foreground S400 on this dark shell.
                .text_color(SurfaceStep::S600.oklch().to_paint())
                .debug_selector(|| TITLEBAR_THREAD_SEPARATOR_SELECTOR.to_owned())
                .child(TITLEBAR_HEADER_THREAD_SEPARATOR)
                .into_any_element(),
            TitlebarHeaderSegment::ThreadTitle { title } => {
                let selector = format!("{TITLEBAR_ROUTE_TITLE_SELECTOR}:{title}");
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .debug_selector(move || selector.clone())
                    .child(SharedString::from((*title).to_owned()))
                    .into_any_element()
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI composition keeps the arrangement in visual order; extraction would split the shared reactive state"
    )]
    pub(super) fn desktop_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let theme = self.desktop_theme;
        let sidebar_item_radius = px(6.0);
        let visible_hover_ids = vec![
            SIDEBAR_NEW_THREAD_HOVER_ID.to_owned(),
            SIDEBAR_MARKETPLACE_HOVER_ID.to_owned(),
            SIDEBAR_PROFILE_HOVER_ID.to_owned(),
        ];
        self.sidebar_hover
            .borrow_mut()
            .clear_if_missing(&visible_hover_ids);

        let sidebar_hover = Rc::clone(&self.sidebar_hover);
        let sidebar_hover_surface_bounds = Rc::clone(&self.sidebar_hover_surface_bounds);
        let surface_bounds = Rc::clone(&sidebar_hover_surface_bounds);
        let surface_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let changed = {
                    let mut surface = surface_bounds.borrow_mut();
                    if *surface == Some(bounds) {
                        false
                    } else {
                        *surface = Some(bounds);
                        true
                    }
                };
                if changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let mut nav_theme = theme;
        nav_theme.secondary = self.theme.colors.muted_foreground.to_paint();
        let nav = div()
            .id("artisan-workspace-navigation")
            .relative()
            .track_focus(&self.sidebar_navigation_focus)
            .tab_index(0)
            .w_full()
            .h(px(34.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .debug_selector(|| "artisan-workspace-navigation".to_owned())
            .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                if *hovered {
                    app.sidebar_hover
                        .borrow_mut()
                        .set_active(SIDEBAR_NEW_THREAD_HOVER_ID.to_owned());
                } else if app.sidebar_hover.borrow().active_id()
                    == Some(SIDEBAR_NEW_THREAD_HOVER_ID)
                {
                    // Hide, don't clear: the retained rect keeps the next
                    // row-to-row flight sliding instead of snapping.
                    app.sidebar_hover.borrow_mut().hide();
                }
                cx.notify();
            }))
            .child(sidebar_hover_probe(
                Rc::clone(&sidebar_hover),
                Rc::clone(&sidebar_hover_surface_bounds),
                SIDEBAR_NEW_THREAD_HOVER_ID,
            ))
            .child(desktop_nav_glyph(AssetId::TABLER_EDIT, nav_theme))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.foreground)
                    .child("New thread"),
            )
            .on_click(cx.listener(|app, _, window, cx| {
                window.focus(&app.sidebar_navigation_focus, cx);
                app.begin_new_task(cx);
            }))
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    app.begin_new_task(cx);
                }
            }));
        let marketplace = div()
            .id("artisan-marketplace-navigation")
            .w_full()
            .h(px(34.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .relative()
            .debug_selector(|| "artisan-marketplace-navigation".to_owned())
            .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                if *hovered {
                    app.sidebar_hover
                        .borrow_mut()
                        .set_active(SIDEBAR_MARKETPLACE_HOVER_ID.to_owned());
                } else if app.sidebar_hover.borrow().active_id()
                    == Some(SIDEBAR_MARKETPLACE_HOVER_ID)
                {
                    app.sidebar_hover.borrow_mut().hide();
                }
                cx.notify();
            }))
            .child(sidebar_hover_probe(
                Rc::clone(&sidebar_hover),
                Rc::clone(&sidebar_hover_surface_bounds),
                SIDEBAR_MARKETPLACE_HOVER_ID,
            ))
            .child(desktop_nav_glyph(AssetId::TABLER_SHOPPING_BAG, nav_theme))
            .child(
                div()
                    .text_size(px(14.0))
                    .text_color(theme.foreground)
                    .child("Marketplace"),
            );
        // One shared hover surface for the whole sidebar column: the same
        // sliding pill travels among New thread, Marketplace, and the
        // profile footer, measured against these bounds. Rows hide the pill
        // on departure and the thread area hides it on entry, all retaining
        // geometry so row-to-row keeps sliding; leaving the column clears
        // it as well.
        let navigation = div()
            .id("artisan-workspace-navigation-hover-surface")
            .relative()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                if !*hovered {
                    app.sidebar_hover.borrow_mut().clear();
                    cx.notify();
                }
            }))
            .child(surface_probe)
            .child(render_picker_hover_pill(
                &self.theme,
                &sidebar_hover,
                "sidebar",
                sidebar_item_radius,
                cx.reduce_motion(),
            ))
            .child(
                div()
                    .id("artisan-sidebar-navigation-scroll")
                    .debug_selector(|| "artisan-sidebar-navigation-scroll".to_owned())
                    .w_full()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(12.0))
                    .child(
                        div()
                            .w_full()
                            .flex_shrink_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(nav)
                            .child(marketplace),
                    )
                    .child(self.desktop_project_switcher(window, cx).flex_shrink_0())
                    .child(self.desktop_sidebar_threads(window, cx)),
            )
            .child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(
                        div()
                            .h(px(1.0))
                            .mx(px(-10.0))
                            .bg(theme.line)
                            .debug_selector(|| "artisan-sidebar-footer-divider".to_owned()),
                    )
                    .child(self.desktop_profile(window, cx)),
            );
        div()
            .h_full()
            .w_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(10.0))
            .child(navigation)
    }

    /// The title the desktop header shows for the current route.
    ///
    /// Thread routes select through the shared display policy: the harness's
    /// generated summary wins for an unlocked title, and the stored listing
    /// title (refined to the latest user message while it is still the
    /// creation placeholder) is the fallback. The new-thread route names the
    /// thread being created once one exists; before then it is the unnamed
    /// draft label.
    pub(super) fn desktop_route_title(&self, cx: &App) -> String {
        match self.route() {
            NativeRoute::NewThread { .. } => self
                .pending_thread
                .as_ref()
                .and_then(|thread| self.listed_thread_display_title(thread, cx))
                .unwrap_or_else(|| UNNAMED_THREAD_TITLE.to_owned()),
            NativeRoute::Thread { thread, .. } => self
                .listed_thread_display_title(thread, cx)
                .unwrap_or_else(|| String::from("Thread")),
            NativeRoute::Editor { .. } => String::from("Files"),
            NativeRoute::Settings { section, .. } => format!("Settings / {}", section.as_str()),
            NativeRoute::Onboarding => String::from("Welcome"),
        }
    }

    /// The route title the desktop header names, when the route carries a
    /// thread subject.
    ///
    /// The bare wordmark stays for routes that do not name a conversation;
    /// once a thread is open â€” or a new-thread route already owns the thread
    /// being created â€” the subject joins the titlebar like the reference's
    /// workspace header.
    pub(super) fn desktop_header_title(&self, cx: &App) -> Option<String> {
        match self.route() {
            NativeRoute::Thread { .. } => Some(self.desktop_route_title(cx)),
            NativeRoute::NewThread { .. } if self.pending_thread.is_some() => {
                Some(self.desktop_route_title(cx))
            }
            _ => None,
        }
    }

    /// Returns the retained harness summary title for `thread`, when one has
    /// arrived on the live engine observation stream.
    pub(super) fn retained_summary_title(&self, thread: &ThreadId) -> Option<String> {
        self.engine_observations
            .as_ref()
            .filter(|state| state.thread_id() == thread)
            .and_then(EngineObservationState::summary_title)
            .map(str::to_owned)
    }

    /// Returns the latest user text from the open conversation for `thread`.
    ///
    /// This is the evidence the reference's live refiner derives its stored
    /// title from. It is only consulted for the exact mounted, selected
    /// thread; any other thread leaves the caller with the stored title.
    pub(super) fn open_thread_latest_user_text(
        &self,
        thread: &ThreadId,
        cx: &App,
    ) -> Option<String> {
        if self.selected_thread.as_ref() != Some(thread) {
            return None;
        }
        let host = self.conversation_host.as_ref()?;
        if &host.read(cx).controller_view().delivery.thread_id != thread {
            return None;
        }
        let snapshot = host.read(cx).canonical_snapshot()?;
        snapshot.items().iter().rev().find_map(|item| match item {
            ConversationItem::UserMessage(message) => Some(message.body.as_str().to_owned()),
            ConversationItem::MultimodalUserMessage(message) => message
                .text
                .as_ref()
                .map(|text| text.as_str().to_owned())
                .filter(|text| !text.trim().is_empty()),
            ConversationItem::AssistantMessage(_) => None,
        })
    }

    /// Selects the display title for one listed thread through the shared
    /// policy.
    ///
    /// The summary comes from the live harness observation for the mounted
    /// thread, the stored title from the authoritative listing, and `false`
    /// for the lock because native rename locking does not exist yet. `None`
    /// means the thread is not listed, so the route supplies its own fallback
    /// label instead.
    pub(super) fn listed_thread_display_title(
        &self,
        thread: &ThreadId,
        cx: &App,
    ) -> Option<String> {
        let item = self
            .thread_listing
            .as_ref()?
            .threads()
            .iter()
            .find(|item| &item.thread_id == thread)?;
        let summary_title = self.retained_summary_title(thread);
        let latest_user_text = self.open_thread_latest_user_text(thread, cx);
        let stored_title = refined_thread_title(item.title.as_str(), latest_user_text.as_deref());
        Some(
            thread_display_title(
                ThreadTitleInput::new(summary_title.as_deref(), stored_title, false),
                ThreadTitleMode::Summary,
            )
            .to_owned(),
        )
    }

    pub(super) fn desktop_route_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let route = self.route().clone();
        let route_selector = route.selector_suffix();
        let content = self.route_surface(window, cx);
        let mut body = div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(shell_black())
            .debug_selector(move || route_selector.clone());
        if matches!(route, NativeRoute::NewThread { .. }) {
            body = body.child(div().flex_1().min_w(px(0.0)).min_h(px(0.0)).child(content));
            body = body.child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .px(px(24.0))
                    .pt(px(12.0))
                    .pb(px(18.0))
                    .flex()
                    .justify_center()
                    .debug_selector(|| DESKTOP_COMPOSER_SELECTOR.to_string())
                    .child(div().w_full().max_w(px(768.0)).child(self.composer.clone())),
            );
        } else {
            body = body.child(content);
        }
        body.into_any_element()
    }
}
