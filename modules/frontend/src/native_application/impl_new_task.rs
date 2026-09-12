//! New-task creation, project intake, and failed-prompt recovery for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-3 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    pub(super) fn begin_new_task(&mut self, cx: &mut Context<Self>) {
        if !self.add_project_action_is_admissible() {
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

    /// Begins explicit new-chat recovery for one exact terminally failed dispatch.
    ///
    /// The failed identity resolves against the generation-fenced queue
    /// projection: never against current composer text or a run id, and only
    /// when the failed row still belongs to the selected thread. The source
    /// composer must be empty and idle first: an inflight submission or an
    /// already-typed draft refuses the action instead of stranding either
    /// prompt. On success the application creates a new thread in the same
    /// project through the existing task flow, seeds the old thread's exact
    /// displayed policy onto the new thread, and recalls the exact failed
    /// payload into the new composer as an unsent draft once the new thread
    /// mounts. Old history and the failed row are preserved; nothing is
    /// autosent.
    pub(super) fn begin_failed_prompt_recovery(
        &mut self,
        command_id: &str,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        let entry = self
            .composer_queue
            .state
            .failed_entries()
            .iter()
            .find(|entry| {
                entry.identity().command_id() == command_id
                    && entry.identity().generation() == generation
            })
            .cloned();
        let Some(entry) = entry else {
            return;
        };
        if self.selected_thread.as_ref() != Some(entry.thread_id()) {
            return;
        }
        if !self.add_project_action_is_admissible() || self.selected_project.is_none() {
            return;
        }
        if self.composer.read(cx).capture_recall_target().is_none() {
            self.message_failure = Some(NativeMessageFailure::new(
                Self::failed_recovery_composer_busy_failure(),
            ));
            self.sync_composer_availability(cx);
            cx.notify();
            return;
        }
        let policy = match &self.composer_model_choice {
            Some((thread, policy)) if thread == &self.selected_thread => Some(policy.clone()),
            _ => self.model_selector.read(cx).state().policy().cloned(),
        };
        self.pending_failed_recovery = Some(PendingFailedRecovery {
            old_thread: entry.thread_id().clone(),
            message_id: entry.message_id().clone(),
            original_request_id: entry.original_request_id().clone(),
            policy,
            new_thread: None,
            recalled: false,
        });
        self.begin_new_task(cx);
        self.sync_composer_controls(cx);
        cx.notify();
    }

    /// Recalls the exact failed payload once the recovery's new thread mounts.
    ///
    /// Fires at most once per pending recovery: the recalled flag fences the
    /// mount hook, and the armed new thread must be the mounted selected
    /// thread. Anything else â€” an unrelated navigation, a missing mount, an
    /// unarmed creation â€” waits or was already cancelled by the navigation
    /// hooks. The old thread's exact displayed policy seeds the new thread
    /// first, so the first send persists the same model/profile instead of
    /// silently resetting to the selector default. A submit failure clears
    /// the pending recovery with a notice while old history and the failed
    /// row stay exactly where they were.
    pub(super) fn continue_failed_recovery(&mut self, cx: &mut Context<Self>) {
        let (old_thread, message_id, original_request_id, policy, new_thread) =
            match self.pending_failed_recovery.as_ref() {
                Some(recovery) if !recovery.recalled => (
                    recovery.old_thread.clone(),
                    recovery.message_id.clone(),
                    recovery.original_request_id.clone(),
                    recovery.policy.clone(),
                    recovery.new_thread.clone(),
                ),
                _ => return,
            };
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        let Some(target_thread) = new_thread else {
            return;
        };
        if selected != target_thread {
            return;
        }
        let mounted = self
            .conversation_host
            .as_ref()
            .is_some_and(|host| host.read(cx).controller_view().delivery.thread_id == selected);
        if !mounted {
            return;
        }
        let scoped_to_new = self
            .composer_model_choice
            .as_ref()
            .is_some_and(|(thread, _)| thread.as_ref() == Some(&selected));
        if !scoped_to_new && let Some(policy) = policy {
            self.composer_model_choice = Some((Some(selected.clone()), policy));
        }
        let query =
            artisan_domain::ReadRecalledMessage::new(old_thread, message_id, original_request_id);
        let command = ComposerStateCommand::ReadRecalledMessage {
            generation: self.composer_queue.state.current_generation(),
            query,
        };
        match self.submit_command(NativeTransportCommand::ComposerState(command)) {
            Ok(()) => {
                if let Some(recovery) = self.pending_failed_recovery.as_mut() {
                    recovery.recalled = true;
                }
            }
            Err(error) => {
                self.pending_failed_recovery = None;
                self.message_failure = Some(NativeMessageFailure::new(command_failure(error)));
                self.sync_composer_availability(cx);
                cx.notify();
            }
        }
    }

    /// Installs one recalled recovery payload into the current composer.
    ///
    /// Returns true exactly when `result` belongs to the pending recovery's
    /// old scope, in which case the pending recovery is always consumed. The
    /// destination must additionally be the armed new thread: anything else
    /// means an unrelated navigation slipped the cancellation hooks, so the
    /// pending recovery drops quietly instead of restoring into the wrong
    /// composer. A present payload restores into the current composer only
    /// while it is still empty: already-typed input is never overwritten,
    /// and the failed row persists on the old thread for an explicit retry.
    /// An absent payload or a refused restore clears the pending recovery
    /// with a notice; nothing is sent anywhere.
    pub(super) fn accept_failed_recovery_result(
        &mut self,
        result: &artisan_domain::RecalledMessageResult,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(recovery) = self.pending_failed_recovery.as_ref() else {
            return false;
        };
        if result.thread_id != recovery.old_thread
            || result.message_id != recovery.message_id
            || result.original_request_id != recovery.original_request_id
        {
            return false;
        }
        let destination = recovery.new_thread.clone();
        self.pending_failed_recovery = None;
        let Some(destination) = destination else {
            return true;
        };
        if self.selected_thread.as_ref() != Some(&destination) {
            return true;
        }
        let Some(payload) = result.payload.clone() else {
            self.message_failure = Some(NativeMessageFailure::new(
                Self::failed_recovery_unreadable_failure(),
            ));
            self.sync_composer_availability(cx);
            cx.notify();
            return true;
        };
        let target = self.composer.read(cx).capture_recall_target();
        let Some(target) = target else {
            self.message_failure = Some(NativeMessageFailure::new(
                Self::failed_recovery_composer_busy_failure(),
            ));
            self.sync_composer_availability(cx);
            cx.notify();
            return true;
        };
        if self
            .composer
            .update(cx, |composer, cx| {
                composer.restore_recalled_payload(&target, payload, cx)
            })
            .is_err()
        {
            self.message_failure = Some(NativeMessageFailure::new(
                Self::failed_recovery_composer_busy_failure(),
            ));
        }
        self.sync_composer_availability(cx);
        cx.notify();
        true
    }

    pub(super) fn failed_recovery_unreadable_failure() -> ServiceFailure {
        ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::Integrity,
        }
    }

    pub(super) fn failed_recovery_composer_busy_failure() -> ServiceFailure {
        ServiceFailure {
            stage: ServiceFailureStage::Request,
            category: ServiceFailureCategory::InvalidConfiguration,
        }
    }

    /// Arms a pending recovery with the intake-resolved thread when a
    /// creation is in flight.
    ///
    /// Intake resolves many listings; only a `CreatingThread` intake can
    /// complete a recovery creation, and manual switches are blocked while
    /// one is in flight. Any other thread never arms the pending recovery,
    /// so a later unrelated navigation cannot inherit it.
    pub(super) fn arm_failed_recovery(&mut self, thread_id: &ThreadId) {
        if self.intake_stage != Some(NativeProjectIntakeStage::CreatingThread) {
            return;
        }
        let Some(recovery) = self.pending_failed_recovery.as_mut() else {
            return;
        };
        if recovery.new_thread.is_some() || *thread_id == recovery.old_thread {
            return;
        }
        recovery.new_thread = Some(thread_id.clone());
    }

    pub(super) fn submit_intake_command(&mut self, cx: &mut Context<Self>) {
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
