//! New-task creation, project intake, and Forge failed-prompt recovery for
//! [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-3 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    pub(super) fn selected_thread_is_draft(&self) -> bool {
        self.selected_thread.as_ref().is_some_and(|selected| {
            self.thread_listing.as_ref().is_some_and(|listing| {
                listing.threads().iter().any(|row| {
                    &row.thread_id == selected
                        && !row.has_started_response
                        && row.last_message_at.is_none()
                        && !row.has_active_work
                })
            })
        })
    }

    pub(super) fn begin_new_task(&mut self, cx: &mut Context<Self>) {
        if !self.add_project_action_is_admissible() {
            return;
        }
        if self.selected_thread_is_draft() {
            self.navigate(
                NativeRoute::NewThread {
                    project: self.selected_project.clone(),
                },
                cx,
            );
            return;
        }
        if self.intake_retry_available || self.selected_project.is_none() {
            self.submit_intake_command(cx);
            if self.intake_stage.is_some() {
                self.navigate(
                    NativeRoute::NewThread {
                        project: self.selected_project.clone(),
                    },
                    cx,
                );
            }
            return;
        }
        let Some(project) = self.selected_project.clone() else {
            return;
        };
        match self.submit_command(NativeTransportCommand::CreateTask(project)) {
            Ok(()) => {
                self.project_navigation.restore_draft = false;
                self.handle_intake_progress(NativeProjectIntakeStage::CreatingThread, cx);
                self.state = NativeViewState::Loading;
                self.navigate(
                    NativeRoute::NewThread {
                        project: self.selected_project.clone(),
                    },
                    cx,
                );
            }
            Err(error) => self.handle_intake_failed(
                NativeProjectIntakeOperation::CreateThread,
                command_failure(error),
                false,
                cx,
            ),
        }
    }

    /// Asks the Forge to move one failed prompt into a new thread of the
    /// selected project.
    ///
    /// The failure resolves against the generation-fenced outbox projection,
    /// never against composer text or a run id, and only while it belongs to
    /// the selected thread. The Forge creates the thread with the failed
    /// message's engine configuration, stores the prompt (images included) as
    /// that thread's draft, and stops offering the failure; the Editor then
    /// opens the thread like a created task, on its Forge draft. Nothing is
    /// sent, and the current composer is left untouched.
    pub(super) fn begin_failed_prompt_recovery(
        &mut self,
        identity: &crate::composer_queue_state::ComposerQueueIdentity,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .composer_queue
            .state
            .failed_entry_for_identity(identity)
            .cloned()
        else {
            return;
        };
        if self.selected_thread.as_ref() != Some(entry.thread_id())
            || !self.add_project_action_is_admissible()
        {
            return;
        }
        let Some(project_id) = self.selected_project.clone() else {
            return;
        };
        let Ok(request_id) = mint_request_id("native-recover") else {
            return;
        };
        let command = NativeTransportCommand::RecoverFailedMessage {
            project_id,
            command: Box::new(artisan_domain::RecoverFailedMessage {
                request_id,
                target: entry.target(),
            }),
        };
        match self.submit_command(command) {
            Ok(()) => {
                // The new thread opens on the draft the Forge stored for it.
                self.intake_opens_forge_draft = true;
                self.project_navigation.restore_draft = true;
                self.handle_intake_progress(NativeProjectIntakeStage::CreatingThread, cx);
                self.state = NativeViewState::Loading;
                self.navigate(
                    NativeRoute::NewThread {
                        project: self.selected_project.clone(),
                    },
                    cx,
                );
            }
            Err(error) => self.handle_intake_failed(
                NativeProjectIntakeOperation::CreateThread,
                command_failure(error),
                false,
                cx,
            ),
        }
    }

    pub(super) fn submit_intake_command(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        if !self.intake_retry_available {
            if let Some(distribution) =
                crate::native_hosts::presentation(self.machine_home.as_deref()).wsl_distribution
            {
                self.choose_wsl_project(distribution, cx);
                return;
            }
        }
        let retryable = self.intake_retry_available;
        match self.submit_command(intake_command(retryable)) {
            Ok(()) => {
                self.retain_message_flight(cx);
                self.clear_message_presentation();
                if self.intake_restore_state.is_none() {
                    self.intake_restore_state = Some(self.state.clone());
                }
                self.intake_stage = Some(NativeProjectIntakeStage::PickingDirectory);
                self.intake_failure_operation = None;
                self.intake_retry_available = false;
                self.state = NativeViewState::Loading;
                self.set_picker_disabled(true, cx);
                cx.notify();
            }
            Err(error) => {
                self.handle_intake_failed(
                    NativeProjectIntakeOperation::PickDirectory,
                    command_failure(error),
                    false,
                    cx,
                );
            }
        }
    }

    #[cfg(test)]
    pub(super) fn activate_add_project(&mut self, cx: &mut Context<Self>) {
        if !self.add_project_action_is_admissible() {
            return;
        }
        self.submit_intake_command(cx);
    }

    #[cfg(test)]
    pub(super) fn add_project_button(&mut self, cx: &mut Context<Self>) -> Button {
        let disabled = !self.add_project_action_is_admissible();
        self.add_project_focus_handle = self.add_project_focus_handle.clone().tab_stop(!disabled);
        let application = cx.entity().downgrade();
        Button::new(
            NATIVE_RAIL_ADD_PROJECT_SELECTOR,
            self.add_project_focus_handle.clone(),
            self.theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::IconSmall,
            ButtonContent::icon_only(
                AssetId::TABLER_FOLDER_PLUS,
                AccessibleLabel::new(NATIVE_RAIL_ADD_PROJECT_LABEL)
                    .expect("the native add-project button has a valid accessible label"),
            ),
        )
        .expect("the native add-project button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .disabled(disabled)
        .debug_selector(NATIVE_RAIL_ADD_PROJECT_SELECTOR)
        .on_activate(move |_, _, app| {
            let _ = application.update(app, |application, cx| {
                application.activate_add_project(cx);
            });
        })
    }
}
