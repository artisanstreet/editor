//! Project and thread picker installation, listing updates, and action routing for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-3 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    pub(super) fn handle_projects(&mut self, listing: &ProjectListing, cx: &mut Context<Self>) {
        if self.thread_switch_flight.is_some() {
            return;
        }
        self.ensure_host_catalog();
        let initial_project = listing
            .projects()
            .first()
            .map(|project| project.project_id.clone());
        let options = self.ordered_project_options(listing);
        // A fresh connection resumes the Forge's last route.
        let listed = |id: &&ProjectId| options.iter().any(|project| &project.id == *id);
        let selected_project = self
            .selected_project
            .as_ref()
            .or_else(|| self.project_navigation.resumed_project())
            .filter(listed)
            .cloned()
            .or_else(|| options.first().map(|project| project.id.clone()));
        if self.selected_project != selected_project || selected_project.is_none() {
            self.retire_host(cx);
            self.pending_thread = None;
            self.pending_snapshot = None;
            self.thread_listing = None;
            self.retained_switch_listings.clear();
            self.install_thread_picker(empty_thread_listing(), None, cx);
        }
        self.project_options.clone_from(&options);
        self.selected_project = selected_project;
        self.install_picker(options.clone(), self.selected_project.clone(), cx);
        self.install_home_picker(options, self.selected_project.clone(), cx);
        if self.project_options.is_empty() {
            self.state = NativeViewState::EmptyProjects;
        } else {
            self.state = NativeViewState::LoadingThreads;
        }
        self.sync_command_menu_groups(cx);
        // Initial transport discovery also reads its first project. A restored
        // selection needs its own read; stale initial replies are ignored.
        if self.selected_project != initial_project
            && let Some(project) = self.selected_project.clone()
            && let Err(error) = self.submit_command(NativeTransportCommand::SelectProject(project))
        {
            self.set_failure(command_failure(error), cx);
        }
        self.request_project_repository(cx);
        cx.notify();
    }

    pub(super) fn install_picker(
        &mut self,
        options: Vec<ProjectOption>,
        current: Option<ProjectId>,
        cx: &mut Context<Self>,
    ) {
        self.install_sidebar_project_picker(options.clone(), current.clone(), cx);
        let picker = cx
            .new(|picker_cx| ProjectPickerView::new(options, current, ThemeMode::Dark, picker_cx));
        let subscription = cx.observe(&picker, |application, picker, cx| {
            application.route_picker_action(&picker, cx);
        });
        self.picker = Some(picker);
        drop(self.picker_subscription.replace(subscription));
        self.last_picker_action = None;
        self.sync_thread_picker_disabled(cx);
    }

    pub(super) fn install_thread_picker(
        &mut self,
        listing: ThreadListing,
        selected_thread: Option<ThreadId>,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.new(|picker_cx| {
            NativeThreadPicker::new(listing, selected_thread, ThemeMode::Dark, picker_cx)
        });
        let subscription = cx.observe(&picker, |application, picker, cx| {
            application.route_thread_picker_action(&picker, cx);
        });
        self.thread_picker = Some(picker);
        drop(self.thread_picker_subscription.replace(subscription));
        self.sync_thread_picker_disabled(cx);
    }

    pub(super) fn update_thread_picker(
        &mut self,
        listing: ThreadListing,
        selected_thread: Option<ThreadId>,
        cx: &mut Context<Self>,
    ) {
        let Some(picker) = self.thread_picker.clone() else {
            self.install_thread_picker(listing, selected_thread, cx);
            return;
        };
        picker.update(cx, |picker, picker_cx| {
            picker.replace_listing(listing, picker_cx);
            picker.set_selected_thread(selected_thread, picker_cx);
        });
        self.sync_thread_picker_disabled(cx);
    }

    pub(super) fn sync_thread_picker_disabled(&mut self, cx: &mut Context<Self>) {
        let disabled = !self.project_picker_action_is_admissible();
        self.set_picker_disabled(disabled, cx);
        self.set_thread_picker_disabled(disabled, cx);
    }

    pub(super) fn set_picker_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if let Some(picker) = self.picker.clone() {
            picker.update(cx, |picker, picker_cx| {
                picker.set_disabled(disabled, picker_cx);
            });
        }
        if let Some(picker) = self.sidebar_project_picker.clone() {
            picker.update(cx, |picker, cx| picker.set_disabled(disabled, cx));
        }
        if let Some(home_picker) = self.home_picker.clone() {
            home_picker.update(cx, |picker, picker_cx| {
                picker.set_disabled(disabled, picker_cx);
            });
        }
    }

    /// Installs the home-surface inline switcher over the same catalog and
    /// current project as the sidebar picker, observed through the shared
    /// picker-action routing.
    pub(super) fn install_home_picker(
        &mut self,
        options: Vec<ProjectOption>,
        current: Option<ProjectId>,
        cx: &mut Context<Self>,
    ) {
        let picker = cx.new(|picker_cx| {
            HomeProjectPickerView::new(options, current, self.desktop_theme, picker_cx)
        });
        let subscription = cx.observe(&picker, |application, picker, cx| {
            application.route_home_picker_action(&picker, cx);
        });
        self.home_picker = Some(picker);
        drop(self.home_picker_subscription.replace(subscription));
        self.sync_thread_picker_disabled(cx);
    }

    pub(super) fn set_thread_picker_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        let Some(picker) = self.thread_picker.clone() else {
            return;
        };
        picker.update(cx, |picker, picker_cx| {
            picker.set_disabled(disabled, picker_cx);
        });
    }

    pub(super) fn route_thread_picker_action(
        &mut self,
        picker: &Entity<NativeThreadPicker>,
        cx: &mut Context<Self>,
    ) {
        let action = picker.update(cx, |picker, _| picker.take_pending_action());
        match action {
            Some(ThreadPickerAction::OpenThread { thread_id }) => {
                self.begin_thread_switch(thread_id, cx);
            }
            None => {}
        }
    }

    pub(super) fn handle_threads(
        &mut self,
        project_id: &ProjectId,
        listing: &ThreadListing,
        cx: &mut Context<Self>,
    ) {
        if self.selected_project.as_ref() != Some(project_id) {
            return;
        }
        let listing_is_valid = listing
            .threads()
            .iter()
            .all(|thread| &thread.project_id == project_id);
        if listing_is_valid && self.project_navigation.awaiting_threads {
            self.project_navigation.awaiting_threads = false;
            let target = self.remembered_project_thread(project_id, listing);
            if let Some(flight) = self.thread_switch_flight.as_mut() {
                // The source unsubscribe and destination read can finish in
                // either order. Bind the target before the stop retires it.
                flight.target_thread.clone_from(&target);
                if let Some(target) = target {
                    self.remember_switch_snapshot_thread(target);
                }
            }
        }
        if self.thread_switch_flight.is_some() {
            self.handle_threads_during_switch(listing_is_valid, listing, cx);
            self.sync_command_menu_groups(cx);
            return;
        }
        self.handle_threads_without_switch(listing_is_valid, project_id, listing, cx);
        self.sync_command_menu_groups(cx);
    }

    pub(super) fn handle_threads_during_switch(
        &mut self,
        listing_is_valid: bool,
        listing: &ThreadListing,
        cx: &mut Context<Self>,
    ) {
        if !listing_is_valid {
            // A project/catalog response from an older project selection
            // cannot mutate a newer thread-switch generation.
            return;
        }
        let source_removed = self.thread_switch_flight.as_ref().is_some_and(|flight| {
            !listing
                .threads()
                .iter()
                .any(|thread| thread.thread_id == flight.source_thread)
        });
        let target_removed = self
            .thread_switch_flight
            .as_ref()
            .and_then(|flight| flight.target_thread.as_ref())
            .is_some_and(|target| {
                !listing
                    .threads()
                    .iter()
                    .any(|thread| &thread.thread_id == target)
            });
        if source_removed || target_removed {
            self.remember_switch_listing();
            self.thread_listing = Some(listing.clone());
            let selected_thread = self.selected_thread.clone();
            self.update_thread_picker(listing.clone(), selected_thread, cx);
            self.pending_snapshot = None;
            if target_removed {
                self.handle_removed_thread_during_switch(cx);
            }
            self.sync_thread_picker_disabled(cx);
            cx.notify();
        }
    }

    pub(super) fn handle_threads_without_switch(
        &mut self,
        listing_is_valid: bool,
        project_id: &ProjectId,
        listing: &ThreadListing,
        cx: &mut Context<Self>,
    ) {
        if !listing_is_valid {
            self.set_failure(invalid_service_failure(), cx);
            return;
        }
        if self
            .thread_listing
            .as_ref()
            .is_some_and(|current| current != listing)
            && self
                .retained_switch_listings
                .iter()
                .any(|retained| retained == listing)
        {
            return;
        }
        let selected_removed = self.selected_thread.as_ref().is_some_and(|selected| {
            !listing
                .threads()
                .iter()
                .any(|thread| &thread.thread_id == selected)
        });
        if selected_removed {
            self.remember_switch_listing();
        }
        self.thread_listing = Some(listing.clone());
        let selected_thread = self.selected_thread.clone();
        self.update_thread_picker(listing.clone(), selected_thread.clone(), cx);
        self.pending_snapshot = None;

        let selected_is_listed = selected_thread.as_ref().is_some_and(|selected| {
            listing
                .threads()
                .iter()
                .any(|thread| &thread.thread_id == selected)
        });
        match (selected_thread, selected_is_listed) {
            (Some(selected_thread), true) => {
                self.pending_thread = self
                    .conversation_host
                    .as_ref()
                    .is_some_and(|host| {
                        host.read(cx).controller_view().delivery.thread_id == selected_thread
                    })
                    .then_some(selected_thread);
                if self.pending_thread.is_some() {
                    self.pending_thread = None;
                    if self
                        .conversation_host
                        .as_ref()
                        .is_some_and(|host| host.read(cx).controller_view().delivery.has_snapshot)
                    {
                        self.state = NativeViewState::Ready;
                    }
                } else {
                    self.state = NativeViewState::Loading;
                    self.try_mount_pending_thread(cx);
                }
            }
            (Some(_), false) => {
                self.pending_thread = None;
                self.update_thread_picker(listing.clone(), None, cx);
                if self.conversation_host.is_some() {
                    self.begin_thread_retirement(cx);
                } else {
                    self.selected_thread = None;
                    self.state = NativeViewState::EmptyThreads;
                    self.sync_thread_picker_selected(cx);
                    self.engine_settings.select_thread(None);
                    self.reset_composer_catalog(cx);
                    self.sync_composer_availability(cx);
                }
            }
            (None, _) => {
                self.pending_thread = self.remembered_project_thread(project_id, listing);
                if self.pending_thread.is_none() {
                    self.state = NativeViewState::EmptyThreads;
                } else {
                    self.state = NativeViewState::Loading;
                    self.try_mount_pending_thread(cx);
                }
            }
        }
        self.sync_thread_picker_disabled(cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn route_picker_action(
        &mut self,
        picker: &Entity<ProjectPickerView>,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = picker.read(cx).last_action() else {
            return;
        };
        self.route_picker_action_inner(&action, cx);
    }

    pub(super) fn route_home_picker_action(
        &mut self,
        picker: &Entity<HomeProjectPickerView>,
        cx: &mut Context<Self>,
    ) {
        let Some(action) = picker.read(cx).last_action() else {
            return;
        };
        self.route_picker_action_inner(&action, cx);
    }

    /// Shared admission-deduped routing for both project picker surfaces.
    pub(super) fn route_picker_action_inner(
        &mut self,
        action: &ProjectPickerAction,
        cx: &mut Context<Self>,
    ) {
        if self.last_picker_action.as_ref() == Some(action) {
            return;
        }
        self.last_picker_action = Some(action.clone());
        if !self.project_picker_action_is_admissible() {
            return;
        }
        match picker_route(action, &self.project_options) {
            Ok(PickerRoute::Select(project_id)) => {
                self.select_project_from_sidebar(project_id, cx);
            }
            Ok(PickerRoute::BeginProjectIntake) => {
                self.submit_intake_command(cx);
            }
            Err(failure) => self.set_failure(failure, cx),
        }
    }
}
