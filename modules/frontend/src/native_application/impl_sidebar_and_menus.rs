//! Sidebar navigation and command-menu flows for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-3 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

#[expect(
    dead_code,
    reason = "settings-open and sidebar-toggle handlers are the typed entry points for the sidebar rail; their mount wiring is owned by the shell/sidebar lane"
)]
impl NativeApplication {
    pub(super) fn sync_command_menu_groups(&mut self, cx: &mut Context<Self>) {
        let mut groups = vec![CommandMenuGroup::actions(), crate::native_hosts::group()];
        if !self.project_options.is_empty() {
            groups.push(CommandMenuGroup::new(
                "projects",
                "Projects",
                self.project_options
                    .iter()
                    .map(|project| {
                        CommandMenuEntry::project(project.id.as_str(), project.name.to_string())
                    })
                    .collect(),
            ));
        }
        if let Some(listing) = self.thread_listing.as_ref() {
            let project_id = self.selected_project.as_ref();
            let entries = listing
                .threads()
                .iter()
                .filter(|thread| {
                    thread.has_started_response && project_id == Some(&thread.project_id)
                })
                .map(|thread| {
                    // The live harness summary is known for the mounted
                    // thread; every other row keeps the stored listing title.
                    let summary_title = self.retained_summary_title(&thread.thread_id);
                    let title = thread_display_title(
                        ThreadTitleInput {
                            summary_title: summary_title.as_deref(),
                            title: thread.title.as_str(),
                            title_locked: false,
                        },
                        ThreadTitleMode::default(),
                    );
                    CommandMenuEntry::thread(
                        thread.thread_id.as_str(),
                        title.to_owned(),
                        thread.title.as_str(),
                    )
                })
                .collect();
            let (group_id, heading) = self.selected_project.as_ref().map_or_else(
                || (String::from("tasks"), String::from("Tasks")),
                |project| {
                    (
                        format!("tasks-{}", project.as_str()),
                        self.selected_project_name()
                            .map_or_else(|| "Tasks".to_owned(), |name| format!("{name} tasks")),
                    )
                },
            );
            groups.push(CommandMenuGroup::new(group_id, heading, entries));
        }
        let command_menu = self.command_menu.clone();
        command_menu.update(cx, |menu, menu_cx| {
            menu.replace_groups(groups, menu_cx);
        });
    }

    pub(super) fn route_command_action(
        &mut self,
        menu: &Entity<NativeCommandMenu>,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = menu.update(cx, |menu, _| menu.take_pending_action()) else {
            return;
        };
        match action {
            CommandMenuAction::AddHost => Self::add_host(cx),
            CommandMenuAction::OpenHost { home } => cx.emit(impl_machines::SelectMachine(home)),
            CommandMenuAction::NewThread => self.begin_new_task(cx),
            CommandMenuAction::OpenSettings => {
                self.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::Models,
                        engine: None,
                    },
                    cx,
                );
            }
            CommandMenuAction::OpenProject { project_id } => {
                let Some(project) = self
                    .project_options
                    .iter()
                    .find(|project| project.id.as_str() == project_id)
                    .map(|project| project.id.clone())
                else {
                    return;
                };
                self.select_project_from_sidebar(project, cx);
            }
            CommandMenuAction::OpenThread { thread_id } => {
                let Some(thread) = self
                    .thread_listing
                    .as_ref()
                    .and_then(|listing| {
                        listing
                            .threads()
                            .iter()
                            .find(|thread| thread.thread_id.as_str() == thread_id)
                    })
                    .map(|thread| thread.thread_id.clone())
                else {
                    return;
                };
                self.open_thread_from_sidebar(thread, cx);
            }
        }
    }

    pub(super) fn activate_command_menu(
        &mut self,
        _: &OpenCommandMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = self.command_menu.clone();
        menu.update(cx, |menu, menu_cx| {
            menu.focus_or_open(window, menu_cx);
        });
    }

    pub(super) fn dismiss_command_menu(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let menu = self.command_menu.clone();
        if menu.read(cx).state().is_open() {
            menu.update(cx, |menu, menu_cx| menu.dismiss(window, menu_cx));
        }
    }

    pub(super) fn activate_settings(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(
            NativeRoute::Settings {
                section: SettingsRoute::Models,
                engine: None,
            },
            cx,
        );
    }

    pub(super) fn toggle_sidebar(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        cx.notify();
    }

    pub(super) fn select_project_from_sidebar(
        &mut self,
        project_id: ProjectId,
        cx: &mut Context<Self>,
    ) {
        self.select_project(project_id, true, cx);
    }

    pub(super) fn select_project(
        &mut self,
        project_id: ProjectId,
        promote: bool,
        cx: &mut Context<Self>,
    ) {
        if !self
            .project_options
            .iter()
            .any(|project| project.id == project_id)
            || !self.project_picker_action_is_admissible()
        {
            return;
        }
        if promote {
            self.promote_project(&project_id);
        }
        if self.selected_project.as_ref() == Some(&project_id) {
            self.sync_project_pickers(cx);
            return;
        }
        if let (Some(project), Some(thread)) = (&self.selected_project, &self.selected_thread) {
            self.project_navigation
                .last_threads
                .insert(project.clone(), thread.clone());
        }
        if (self.selected_thread_is_draft() || self.composer.read(cx).has_unsent_draft())
            && self.command_submission_is_available()
            && !self.composer.read(cx).is_submitting()
            && self.message_flight.is_none()
        {
            // The unsent prompt follows the workspace into a fresh task,
            // without replacing a destination conversation's saved draft.
            self.selected_project = Some(project_id.clone());
            self.intake_failure_operation = None;
            self.intake_retry_available = false;
            self.intake_restore_state = None;
            self.thread_listing = None;
            self.pending_thread = None;
            self.pending_snapshot = None;
            self.retained_switch_listings.clear();
            self.sync_project_pickers(cx);
            self.install_thread_picker(empty_thread_listing(), None, cx);
            self.navigate(
                NativeRoute::NewThread {
                    project: Some(project_id),
                },
                cx,
            );
            self.begin_new_task(cx);
            self.request_project_repository(cx);
            return;
        }
        self.retain_message_flight(cx);
        self.clear_message_presentation();
        self.project_navigation.restore_draft = true;
        self.composer.update(cx, |composer, cx| {
            composer.switch_thread(&format!("project:{}", project_id.as_str()), false, cx);
        });
        self.intake_stage = None;
        self.intake_failure_operation = None;
        self.intake_retry_available = false;
        self.intake_restore_state = None;
        self.pending_thread = None;
        self.pending_snapshot = None;
        self.project_navigation.awaiting_threads = true;
        if self.selected_project.as_ref() != Some(&project_id) {
            if self.conversation_host.is_some() && self.selected_thread.is_some() {
                // Keep the source subscription until its stop receipt. The
                // destination listing supplies this transition's target.
                self.begin_thread_retirement(cx);
            } else {
                self.retire_host(cx);
            }
            self.thread_listing = None;
            self.retained_switch_listings.clear();
            self.install_thread_picker(empty_thread_listing(), None, cx);
        }
        self.selected_project = Some(project_id.clone());
        self.sync_project_pickers(cx);
        self.navigate(
            NativeRoute::NewThread {
                project: Some(project_id.clone()),
            },
            cx,
        );
        self.set_thread_picker_disabled(true, cx);
        self.state = NativeViewState::Loading;
        self.sync_command_menu_groups(cx);
        self.sync_composer_availability(cx);
        match self.submit_command(NativeTransportCommand::SelectProject(project_id)) {
            Ok(()) => cx.notify(),
            Err(error) => self.set_failure(command_failure(error), cx),
        }
        self.request_project_repository(cx);
    }

    pub(super) fn open_thread_from_sidebar(&mut self, thread_id: ThreadId, cx: &mut Context<Self>) {
        if !self.thread_is_listed(&thread_id) || !self.project_picker_action_is_admissible() {
            return;
        }
        let Some(project) = self.selected_project.clone() else {
            return;
        };
        if self.selected_thread.as_ref() == Some(&thread_id) {
            self.navigate(
                NativeRoute::Thread {
                    project,
                    thread: thread_id,
                },
                cx,
            );
            self.sync_composer_availability(cx);
            return;
        }
        if self.selected_thread.is_none() || self.conversation_host.is_none() {
            self.selected_thread = None;
            self.pending_thread = Some(thread_id);
            self.state = NativeViewState::Loading;
            self.try_mount_pending_thread(cx);
        } else {
            self.begin_thread_switch(thread_id, cx);
        }
        self.sync_composer_availability(cx);
        cx.notify();
    }
}
