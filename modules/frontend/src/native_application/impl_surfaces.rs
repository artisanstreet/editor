//! Home, titlebar, sidebar, and desktop route surface composition for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-3 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;
use crate::native_composer::{ComposerGoal, ComposerGoalState, ComposerTab};
use crate::thread_screen::{
    COLUMN_PAD_X_PX, PROSE_WIDTH_PX, composer_pad_bottom, inspector_column, thread_inspector_width,
};
use artisan_domain::{GoalState, SafetyAction, SafetyCategory};
use gpui::{canvas, prelude::FluentBuilder as _};

impl NativeApplication {
    /// Renders what sits above the new-thread composer: nothing while the
    /// Forge is reachable, since the composer and its project tab say it all.
    /// The reconnecting, add-a-host, and offline branches keep their emblem
    /// and actionable copy.
    pub(super) fn new_thread_surface_section(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let section = || {
            div()
                .flex()
                .flex_col()
                .items_center()
                .debug_selector(|| DESKTOP_HOME_SELECTOR.to_string())
        };
        // Status branches only: the emblem, and room before the composer.
        let root = section().pb(px(32.0)).child(self.home_emblem());
        if self.connection_retry_pending {
            return root.child(self.home_heading("Reconnecting to Forge…"));
        }
        if self.service.is_none() && self.machine_home.is_none() {
            return root
                .child(self.home_heading("Add a host"))
                .child(
                    div()
                        .mt(px(8.0))
                        .max_w(px(440.0))
                        .text_size(px(15.0))
                        .text_color(self.desktop_theme.secondary)
                        .child(
                            "Artisan works on a Forge host you trust. Select the invitation the host published to connect to it.",
                        ),
                )
                .child(
                    div()
                        .id("add-forge-host")
                        .debug_selector(|| "add-forge-host".into())
                        .cursor_pointer()
                        .mt(px(16.0))
                        .px(px(16.0))
                        .py(px(8.0))
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(self.desktop_theme.line)
                        .child("Add a host")
                        .on_click(cx.listener(|_, _, _, cx| Self::add_host(cx))),
                );
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
        section()
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
        self.project_repository = None;
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

    /// Retains one inspected repository observation for the titlebar header
    /// and the thread inspector's Branch row.
    ///
    /// A reply for a project that is no longer selected is dropped: the
    /// selection fence, not arrival order, decides what the header and the
    /// inspector show.
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
        self.project_repository = repository.map(environment_repository_for_project);
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
            self.project_repository = None;
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
        // Wordmark at the start, build id at the end, both centred on the
        // titlebar's height.
        let brand = div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .min_w(px(0.0))
            .overflow_hidden()
            .child(
                div()
                    .id("artisan-brand-home")
                    .block_mouse_except_scroll()
                    .line_height(px(28.0))
                    .cursor_pointer()
                    .on_click(cx.listener(|app, _, _, cx| app.go_home(cx)))
                    .debug_selector(|| "artisan-brand-home".to_owned())
                    .flex_shrink_0()
                    .text_size(px(26.0))
                    .font_family("Test Founders Grotesk")
                    .font_weight(FontWeight::SEMIBOLD)
                    // -0.025em tracking matches the Founders specimen.
                    .letter_spacing(px(-0.65))
                    .text_color(self.desktop_theme.foreground)
                    .child("Artisan Editor"),
            );
        // The build id as a glass badge, without its separator.
        match artisan_build_info::BuildIdentity::current().badge() {
            Some(badge) => brand.child(
                self.glass_badge(badge.replace('-', ""))
                    .debug_selector(|| "artisan-brand-build".to_owned()),
            ),
            None => brand,
        }
    }

    /// The titlebar workspace header, when the route names a conversation.
    ///
    /// The line follows the reference strip: a repository host mark and its
    /// qualified `owner/repository` link when inspected repository facts
    /// exist, the project folder otherwise, then the darker `/` separator and
    /// the conversation title. A route that names no conversation returns
    /// `None`, leaving the bare wordmark.
    pub(super) fn desktop_header_cluster(&self) -> Option<AnyElement> {
        let thread_title = self.desktop_header_title()?;
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
                // The folder fallback has no link to tint, so its label
                // carries the foreground; the icon keeps the muted context
                // tone.
                .text_color(theme.colors.foreground.to_paint())
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
        let marketplace_open = matches!(self.route(), NativeRoute::Marketplace);
        let marketplace = div()
            .id("artisan-marketplace-navigation")
            .track_focus(&self.sidebar_marketplace_focus)
            .tab_index(0)
            .cursor_pointer()
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
                    // The open Marketplace is marked like the open thread:
                    // a link-coloured title, never the hover fill.
                    .text_color(if marketplace_open {
                        self.theme.colors.banner_info.to_paint()
                    } else {
                        theme.foreground
                    })
                    .child("Marketplace"),
            )
            .on_click(cx.listener(|app, _, window, cx| {
                window.focus(&app.sidebar_marketplace_focus, cx);
                app.navigate(NativeRoute::Marketplace, cx);
            }))
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, _, cx| {
                if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    cx.stop_propagation();
                    app.navigate(NativeRoute::Marketplace, cx);
                }
            }));
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
                            .mx(px(-crate::desktop_shell::DESKTOP_COLUMN_INSET_PX))
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
            .p(px(crate::desktop_shell::DESKTOP_COLUMN_INSET_PX))
            .child(navigation)
    }

    /// The title the desktop header shows for the current route.
    ///
    /// Thread routes show the Forge's resolved display title from the
    /// listing. The new-thread route names the thread being created once one
    /// exists; before then it is the unnamed draft label.
    pub(super) fn desktop_route_title(&self) -> String {
        match self.route() {
            NativeRoute::NewThread { .. } => self
                .pending_thread
                .as_ref()
                .and_then(|thread| self.listed_thread_display_title(thread))
                .unwrap_or_else(|| UNNAMED_THREAD_TITLE.to_owned()),
            NativeRoute::Thread { thread, .. } => self
                .listed_thread_display_title(thread)
                .unwrap_or_else(|| String::from("Thread")),
            NativeRoute::Editor { .. } => String::from("Files"),
            NativeRoute::Settings { section, .. } => format!("Settings / {}", section.as_str()),
            NativeRoute::Onboarding => String::from("Welcome"),
            NativeRoute::Marketplace => String::from("Marketplace"),
        }
    }

    /// The route title the desktop header names, when the route carries a
    /// thread subject.
    ///
    /// The bare wordmark stays for routes that do not name a conversation;
    /// once a thread is open â€” or a new-thread route already owns the thread
    /// being created â€” the subject joins the titlebar like the reference's
    /// workspace header.
    pub(super) fn desktop_header_title(&self) -> Option<String> {
        match self.route() {
            NativeRoute::Thread { .. } => Some(self.desktop_route_title()),
            NativeRoute::NewThread { .. } if self.pending_thread.is_some() => {
                Some(self.desktop_route_title())
            }
            _ => None,
        }
    }

    /// Returns the display title of one listed thread.
    ///
    /// The Forge resolves it (the harness-generated title once recorded,
    /// otherwise the first message while the thread still carries its
    /// creation placeholder) and the listing carries the result. `None`
    /// means the thread is not listed, so the route supplies its own
    /// fallback label instead.
    pub(super) fn listed_thread_display_title(&self, thread: &ThreadId) -> Option<String> {
        self.thread_listing
            .as_ref()?
            .threads()
            .iter()
            .find(|item| &item.thread_id == thread)
            .map(|item| item.title.as_str().to_owned())
    }

    pub(super) fn desktop_route_body(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let route = self.route().clone();
        let route_selector = route.selector_suffix();
        self.sync_composer_tab(&route, cx);
        let content = self.route_surface(window, cx);
        let body = div()
            .size_full()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .bg(shell_black())
            .debug_selector(move || route_selector.clone());
        if matches!(route, NativeRoute::NewThread { .. }) {
            // Typed text reaches only the focused surface: entering the
            // screen focuses the composer once, later renders never steal it.
            if std::mem::take(&mut self.new_thread_focus_pending) {
                let focus = self.composer.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            }
            body.child(self.new_thread_layout(content, window))
                .into_any_element()
        } else {
            body.child(content).into_any_element()
        }
    }

    /// Points the composer's tab at the route: the project picker on the
    /// new-thread screen, the thread's own project inside a thread.
    fn sync_composer_tab(&mut self, route: &NativeRoute, cx: &mut Context<Self>) {
        let tab = match route {
            NativeRoute::NewThread { .. } => {
                let label: SharedString = self
                    .selected_project_name()
                    .unwrap_or_else(|| HOME_CHOOSE_PROJECT_LABEL.to_owned())
                    .into();
                Some(match self.home_picker.clone() {
                    Some(picker) => ComposerTab::Picker { picker, label },
                    None => ComposerTab::Project(label),
                })
            }
            NativeRoute::Thread { project, .. } => self
                .project_options
                .iter()
                .find(|option| &option.id == project)
                .map(|option| ComposerTab::Project(option.name.to_string().into())),
            _ => None,
        };
        let goal = match route {
            NativeRoute::Thread { .. } => self.composer_goal(cx),
            _ => None,
        };
        self.ensure_extensions_read();
        let mentions = self.composer_mention_catalog(cx);
        if cx
            .try_global::<crate::composer_mentions::SharedMentionCatalog>()
            .is_none_or(|shared| shared.0 != mentions)
        {
            cx.set_global(crate::composer_mentions::SharedMentionCatalog(
                mentions.clone(),
            ));
        }
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_tab(tab, composer_cx);
            composer.set_goal(goal, composer_cx);
            composer.set_mention_catalog(mentions, composer_cx);
        });
    }

    /// The engine the composer's displayed model runs on.
    fn composer_engine_id(&self, cx: &App) -> Option<String> {
        self.composer_model_choice(cx)
            .or_else(|| self.model_selector.read(cx).state().policy())
            .map(|policy| policy.engine_id.clone())
    }

    /// What the composer can name: the skills the Forge reports in the
    /// displayed model's engine, and the commands that engine reads.
    fn composer_mention_catalog(&self, cx: &App) -> crate::composer_mentions::MentionCatalog {
        let engine = self.composer_engine_id(cx);
        let project = match self.route() {
            NativeRoute::Thread { project, .. } => Some(project.clone()),
            _ => self.selected_project.clone(),
        };
        crate::composer_mentions::catalog_for_engine(
            self.extensions_catalog(),
            engine.as_deref(),
            project.as_ref().map(artisan_domain::ProjectId::as_str),
        )
    }

    /// The open thread's goal as the composer shows it: only a goal that
    /// still stands. One met, given up, or cleared simply goes.
    ///
    /// A provider safety system acting on the current run takes the strip
    /// first, naming the engine: the reader must learn why the turn stopped
    /// or changed model before what it was working toward.
    fn composer_goal(&self, cx: &App) -> Option<ComposerGoal> {
        let observations = self.engine_observations.as_ref()?;
        if let Some(safety) = observations.safety() {
            let engine = self.composer_engine_id(cx);
            let engine =
                crate::marketplace_presentation::engine_name(engine.as_deref().unwrap_or(""));
            if let Some(notice) = composer_safety_notice(engine, safety) {
                return Some(notice);
            }
        }
        let goal = observations.goal()?;
        let state = match goal.state() {
            GoalState::Active => ComposerGoalState::Active,
            GoalState::Paused => ComposerGoalState::Paused,
            GoalState::Met | GoalState::Abandoned | GoalState::Cleared => return None,
        };
        Some(ComposerGoal {
            objective: goal.objective().unwrap_or("Goal").to_owned().into(),
            state,
            detail: None,
        })
    }

    /// The new-thread screen in the thread screen's own geometry: the
    /// conversation column beside an empty inspector column, the composer
    /// docked exactly where a thread docks it but lifted to the column's
    /// vertical centre, with the heading above it. The lift is measured each
    /// layout and handed to the thread a send opens, which slides the
    /// composer from here into its dock.
    fn new_thread_layout(&self, heading: AnyElement, window: &Window) -> Div {
        let viewport_width = f32::from(window.bounds().size.width);
        let shell = self.desktop_shell_style(window);
        let pad_bottom = composer_pad_bottom(viewport_width);
        let lift_cell = Rc::clone(&self.new_thread_composer_lift);
        let lift = lift_cell.get();
        let column_height = Rc::new(Cell::new(0.0_f32));
        let column_probe = Rc::clone(&column_height);

        let overlay = div()
            .absolute()
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(lift.unwrap_or(0.0)))
            .flex()
            .flex_col()
            .items_center()
            .px(px(COLUMN_PAD_X_PX))
            .pb(px(pad_bottom))
            // Unmeasured, the composer would flash at the dock for a frame.
            .when(lift.is_none(), |overlay| overlay.opacity(0.0))
            .on_children_prepainted(move |children, window, app| {
                let Some(card) = children.last() else {
                    return;
                };
                let centred = (column_height.get() - f32::from(card.size.height)) / 2.0;
                let next = (centred - pad_bottom).max(0.0);
                if lift_cell
                    .get()
                    .is_none_or(|current| (current - next).abs() > 0.5)
                {
                    lift_cell.set(Some(next));
                    window.defer(app, |window, _| window.refresh());
                }
            })
            .child(heading)
            .child(
                div()
                    .w_full()
                    .max_w(px(PROSE_WIDTH_PX))
                    .debug_selector(|| DESKTOP_COMPOSER_SELECTOR.to_string())
                    .child(self.composer.clone()),
            );
        let conversation = div()
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .child(
                canvas(
                    move |bounds, _, _| column_probe.set(f32::from(bounds.size.height)),
                    |_, (), _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(overlay);
        let content_width = viewport_width - f32::from(shell.sidebar_width);
        div()
            .flex()
            .flex_row()
            .flex_1()
            .min_h(px(0.0))
            .child(conversation)
            .children(
                thread_inspector_width(content_width)
                    .map(|width| inspector_column(width, shell.one_device_pixel)),
            )
    }
}

/// The strip's notice for a provider safety report; `None` once the check
/// released the response.
fn composer_safety_notice(
    engine: &str,
    safety: &artisan_domain::SafetyObservation,
) -> Option<ComposerGoal> {
    let area = match safety.category() {
        SafetyCategory::Cyber => Some("cybersecurity"),
        SafetyCategory::Bio => Some("biology"),
        SafetyCategory::Chemical => Some("chemistry"),
        SafetyCategory::Policy => Some("usage policy"),
        SafetyCategory::Unspecified => None,
    };
    let (headline, state) = match safety.action() {
        SafetyAction::Released => return None,
        SafetyAction::Holding => (
            format!("{engine} is running a safety check on this request"),
            ComposerGoalState::SafetyHolding,
        ),
        SafetyAction::Rerouted => (
            match safety.model() {
                Some(model) => format!("{engine} moved this request to {model}"),
                None => format!("{engine} moved this request to a fallback model"),
            },
            ComposerGoalState::SafetyRerouted,
        ),
        SafetyAction::Refused => (
            format!("{engine} refused to serve this request"),
            ComposerGoalState::SafetyRefused,
        ),
    };
    let headline = match area {
        Some(area) => format!("{headline} ({area})"),
        None => headline,
    };
    Some(ComposerGoal {
        objective: headline.into(),
        state,
        detail: safety.message().map(|message| message.to_owned().into()),
    })
}

#[cfg(test)]
mod safety_notice_tests {
    use super::*;
    use artisan_domain::{ObservationId, ObservationSequence, SafetyObservation};

    fn safety(
        action: SafetyAction,
        category: SafetyCategory,
        model: Option<&str>,
    ) -> SafetyObservation {
        SafetyObservation::new(
            ObservationId::parse("safety-1").expect("valid id"),
            ObservationSequence::new(1).expect("valid sequence"),
            action,
            category,
            None,
            Some("Flagged for review.".to_owned()),
            model.map(str::to_owned),
        )
        .expect("valid safety row")
    }

    #[test]
    fn a_refusal_names_the_engine_and_the_area() {
        let notice = composer_safety_notice(
            "Codex",
            &safety(SafetyAction::Refused, SafetyCategory::Cyber, None),
        )
        .expect("a refusal shows");
        assert_eq!(
            notice.objective.as_ref(),
            "Codex refused to serve this request (cybersecurity)"
        );
        assert_eq!(notice.state, ComposerGoalState::SafetyRefused);
        assert_eq!(notice.detail.as_deref(), Some("Flagged for review."));
    }

    #[test]
    fn a_reroute_names_the_fallback_model_and_a_release_shows_nothing() {
        let notice = composer_safety_notice(
            "Codex",
            &safety(
                SafetyAction::Rerouted,
                SafetyCategory::Cyber,
                Some("gpt-5.2"),
            ),
        )
        .expect("a reroute shows");
        assert_eq!(
            notice.objective.as_ref(),
            "Codex moved this request to gpt-5.2 (cybersecurity)"
        );
        let unspecified = composer_safety_notice(
            "Grok",
            &safety(SafetyAction::Refused, SafetyCategory::Unspecified, None),
        )
        .expect("a refusal shows");
        assert_eq!(
            unspecified.objective.as_ref(),
            "Grok refused to serve this request"
        );
        assert_eq!(
            composer_safety_notice(
                "Codex",
                &safety(SafetyAction::Released, SafetyCategory::Cyber, None)
            ),
            None
        );
    }
}
