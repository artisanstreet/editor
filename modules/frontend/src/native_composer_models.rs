use super::*;
use crate::native_model_selector::{
    NativeModelSelectorEvent, NativeModelSelectorStatus, SetFavorite,
};
use crate::native_transport::FavoriteIntentError;

impl NativeApplication {
    pub(super) fn handle_composer_model_event(
        &mut self,
        event: &NativeModelSelectorEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            NativeModelSelectorEvent::RefreshCatalog => {
                self.ensure_profile_usage(false, None, cx);
                if let Some(scope) = self.catalog_controller.refresh_catalog() {
                    self.submit_composer_catalog_reads(&scope, cx);
                } else if self.catalog_controller.scope().is_none() {
                    self.discover_composer_catalog_for_settings(cx);
                }
            }
            NativeModelSelectorEvent::Retry => {
                // The retry behind every composer error refreshes the Forge's
                // account verdict first, so a recovered sign-in or repaired
                // binary is observed instead of re-failing on a stale row.
                self.ensure_profile_usage(false, None, cx);
                if let Some(pending) = self.catalog_controller.pending_favorite().cloned() {
                    if !pending.admitted {
                        self.submit_pending_model_favorite(&pending, cx);
                    }
                } else if self.engine_settings.can_save() {
                    self.save_engine_settings(cx);
                } else if !self.retry_policy_save(cx) {
                    self.composer_model_run_error = None;
                    self.retry_composer_catalog(cx);
                    self.retry_composer_favorites(cx);
                }
            }
            NativeModelSelectorEvent::SelectPolicy(policy) => {
                self.handle_composer_policy_selection(policy, cx);
            }
            NativeModelSelectorEvent::SetFavorite(intent) => {
                self.handle_composer_favorite(intent, cx);
            }
        }
        self.refresh_settings_engine_snapshot(cx);
    }

    pub(super) fn handle_composer_policy_selection(
        &mut self,
        policy: &crate::native_model_selector::SelectPolicy,
        cx: &mut Context<Self>,
    ) {
        // The Forge makes the configuration a choice saves the default new
        // threads start from, and pushes it.
        self.composer_model_choice = Some((self.selected_thread.clone(), policy.clone()));
        self.composer_model_run_error = None;
        self.sync_composer_controls(cx);
        let Some(thread) = self.selected_thread.clone() else {
            return;
        };
        if self.engine_settings.pending_save_request_id().is_some() {
            self.deferred_composer_policy = Some((thread, policy.clone()));
            return;
        }
        self.deferred_composer_policy = None;
        self.request_selection_resolution(thread, policy);
        self.sync_composer_model_policy(cx);
    }

    /// Asks the Forge to resolve the displayed choice into the configuration
    /// it would run; the answer is saved through the shared direct save.
    pub(super) fn request_selection_resolution(
        &mut self,
        thread_id: ThreadId,
        policy: &crate::native_model_selector::SelectPolicy,
    ) {
        let Some(selection) = crate::picker_selection::selection_for_policy(policy) else {
            return;
        };
        self.pending_resolution = Some((thread_id.clone(), selection.clone()));
        let command = NativeTransportCommand::ForgeDecision(
            crate::native_transport_service::ForgeDecisionCommand::ResolveModelSelection(
                artisan_domain::ResolveModelSelection {
                    thread_id,
                    selection,
                },
            ),
        );
        if self.submit_command(command).is_err() {
            self.pending_resolution = None;
            self.composer_model_run_error = Some(SELECTION_SAVE_FAILED.to_owned());
        }
    }

    /// Applies the Forge's resolution of the latest selection: the resolved
    /// configuration is saved unless it is already the thread's; a refusal
    /// is shown as the Forge worded it.
    pub(super) fn receive_selection_resolution(
        &mut self,
        thread_id: ThreadId,
        selection: artisan_domain::CatalogSelection,
        result: Result<
            Result<Box<artisan_domain::EngineRunConfig>, artisan_domain::SubmissionRefusal>,
            ServiceFailure,
        >,
        cx: &mut Context<Self>,
    ) {
        if self.pending_resolution.as_ref() != Some(&(thread_id.clone(), selection)) {
            return;
        }
        self.pending_resolution = None;
        if self.selected_thread.as_ref() != Some(&thread_id) {
            return;
        }
        match result {
            Ok(Ok(config)) => {
                if self.engine_settings.pending_save_request_id().is_some() {
                    // A save began meanwhile; its acknowledgement resolves
                    // the latest choice again.
                    self.deferred_composer_policy = self
                        .composer_model_choice
                        .as_ref()
                        .filter(|(thread, _)| thread.as_ref() == Some(&thread_id))
                        .map(|(_, choice)| (thread_id.clone(), choice.clone()));
                } else if self.engine_settings.authoritative_config() != Some(&*config)
                    && !self.submit_direct_save(thread_id, *config)
                {
                    self.composer_model_run_error = Some(SELECTION_SAVE_FAILED.to_owned());
                }
            }
            Ok(Err(refusal)) => {
                self.composer_model_run_error = Some(refusal.message().to_owned());
            }
            Err(_) => self.composer_model_run_error = Some(SELECTION_SAVE_FAILED.to_owned()),
        }
        self.sync_composer_model_policy(cx);
        self.sync_composer_controls(cx);
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    /// Retries the displayed choice for the selected thread through the Forge
    /// resolution and the shared direct save, so the retry behind a composer
    /// error recovers a stranded selection exactly like the original one.
    /// Returns whether a resolution was requested.
    fn retry_policy_save(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(thread_id) = self.selected_thread.clone() else {
            return false;
        };
        if self.engine_settings.pending_save_request_id().is_some() {
            return false;
        }
        let Some((choice_thread, choice)) = self.composer_model_choice.clone() else {
            return false;
        };
        if choice_thread != self.selected_thread {
            return false;
        }
        self.composer_model_run_error = None;
        self.request_selection_resolution(thread_id, &choice);
        self.sync_composer_model_policy(cx);
        cx.notify();
        true
    }

    fn handle_composer_favorite(&mut self, intent: &SetFavorite, cx: &mut Context<Self>) {
        if let Some(pending) = self.catalog_controller.pending_favorite().cloned() {
            if pending.model_id.as_str() == intent.model_id && pending.favorite == intent.favorite {
                if pending.admitted {
                    self.sync_composer_catalog_status(cx);
                } else {
                    self.submit_pending_model_favorite(&pending, cx);
                }
            } else {
                self.set_model_selector_error(
                    "A model favorite update is still pending. Retry it before changing another favorite.",
                    cx,
                );
            }
            return;
        }

        let Some(scope) = self.catalog_controller.scope().cloned() else {
            self.set_model_selector_error(
                "Runtime model catalog is unavailable until a configured thread is loaded.",
                cx,
            );
            return;
        };
        if !self.catalog_controller.catalog_ready() {
            self.set_model_selector_error(
                "Runtime model catalog is still unavailable; favorites cannot be changed offline.",
                cx,
            );
            return;
        }
        let catalog = self.model_selector.read(cx).state().snapshot();
        if catalog.manifest.model(&intent.model_id).is_none() {
            self.set_model_selector_error("The selected model is not in the runtime catalog.", cx);
            return;
        }
        let Ok(model_id) = ModelFavoriteId::parse(intent.model_id.clone()) else {
            self.set_model_selector_error("The selected model identity is invalid.", cx);
            return;
        };
        let Ok(catalog_revision) = CatalogRevision::parse(catalog.catalog_revision.clone()) else {
            self.set_model_selector_error(
                "The runtime catalog revision is invalid; reload the catalog before retrying.",
                cx,
            );
            return;
        };
        let Ok(request_id) = create_model_favorite_request_id() else {
            self.set_model_selector_error("Favorite request identity is unavailable.", cx);
            return;
        };
        let pending = match self.catalog_controller.begin_favorite(
            &scope,
            request_id,
            model_id,
            catalog_revision,
            intent.favorite,
        ) {
            Ok(pending) => pending,
            Err(FavoriteIntentError::CatalogUnavailable) => {
                self.set_model_selector_error(
                    "Runtime model catalog is unavailable; favorites cannot be changed offline.",
                    cx,
                );
                return;
            }
            Err(FavoriteIntentError::AlreadyPending) => return,
        };
        self.submit_pending_model_favorite(&pending, cx);
    }

    pub(super) fn submit_pending_model_favorite(
        &mut self,
        pending: &crate::native_transport::PendingModelFavorite,
        cx: &mut Context<Self>,
    ) {
        let command = SetModelFavorite::new(
            pending.request_id.clone(),
            pending.scope.thread_id.clone(),
            pending.scope.profile_id.clone(),
            pending.catalog_revision.clone(),
            pending.model_id.clone(),
            pending.favorite,
        );
        let Some(service) = self.service.clone() else {
            let _ = self.catalog_controller.on_favorite_admission_failed(
                &pending.scope,
                &pending.request_id,
                NativeCatalogController::unavailable_failure(),
            );
            self.sync_composer_catalog_status(cx);
            return;
        };
        match service.submit(NativeTransportCommand::SetModelFavorite(Box::new(command))) {
            Ok(()) => {
                if !self
                    .catalog_controller
                    .mark_favorite_admitted(&pending.scope, &pending.request_id)
                {
                    let _ = self.catalog_controller.on_favorite_admission_failed(
                        &pending.scope,
                        &pending.request_id,
                        invalid_service_failure(),
                    );
                }
            }
            Err(error) => {
                let _ = self.catalog_controller.on_favorite_admission_failed(
                    &pending.scope,
                    &pending.request_id,
                    command_failure(error),
                );
            }
        }
        self.sync_composer_catalog_status(cx);
    }

    fn set_model_selector_error(&mut self, message: &str, cx: &mut Context<Self>) {
        let saving = self.engine_settings.pending_save_request_id().is_some()
            || self.catalog_controller.catalog_loading()
            || self
                .catalog_controller
                .pending_favorite()
                .is_some_and(|pending| pending.admitted);
        self.model_selector.update(cx, |selector, cx| {
            selector.set_status(
                NativeModelSelectorStatus {
                    saving,
                    error: Some(message.to_owned()),
                    authoritative: false,
                },
                cx,
            );
        });
    }

    /// Shows the Forge's default configuration as the catalog row that
    /// carries it.
    ///
    /// Only for threads with no per-thread choice and no saved
    /// configuration: saved threads never reach this fallback, and the
    /// result stays non-authoritative until the thread saves its own.
    fn default_display_policy(
        &self,
        snapshot: &crate::native_model_catalog::NativeModelCatalog,
    ) -> Option<crate::native_model_catalog::NativeModelPolicy> {
        let config = self.default_engine_config.as_ref()?;
        crate::picker_selection::saved_config_policy(snapshot, config)
    }

    pub(super) fn sync_composer_model_policy(&mut self, cx: &mut Context<Self>) {
        if self
            .composer_model_choice
            .as_ref()
            .is_some_and(|(thread, _)| thread != &self.selected_thread)
        {
            self.composer_model_choice = None;
            self.composer_model_run_error = None;
        }
        // The saved configuration displays as the catalog row and options
        // that carry it; the Editor never rebuilds a configuration.
        let snapshot = self.served_catalog(cx);
        let saved_policy = self
            .engine_settings
            .authoritative_config()
            .and_then(|config| crate::picker_selection::saved_config_policy(&snapshot, config));
        if let Some((thread, choice)) = self.deferred_composer_policy.as_mut()
            && Some(&*thread) == self.selected_thread.as_ref()
            && let Some(rebased) = snapshot.rebase_policy(choice)
        {
            *choice = rebased;
        }

        if let Some((_, choice)) = self.composer_model_choice.as_mut()
            && let Some(rebased) = snapshot.rebase_policy(choice)
        {
            *choice = rebased;
        }

        let policy = self
            .composer_model_choice
            .as_ref()
            .and_then(|(_, choice)| snapshot.rebase_policy(choice))
            .or_else(|| saved_policy.clone())
            .or_else(|| self.default_display_policy(&snapshot));
        // The displayed choice is authoritative exactly when it is the saved
        // configuration's own display.
        let authoritative = policy.is_some() && policy == saved_policy;
        let pending_save = self.engine_settings.pending_save_request_id().is_some();
        let saving = pending_save
            || self.pending_resolution.is_some()
            || self.catalog_controller.catalog_loading()
            || self
                .catalog_controller
                .pending_favorite()
                .is_some_and(|pending| pending.admitted);
        let error = if self.engine_settings.failure_operation()
            == Some(crate::engine_settings::EngineSettingsFailureOperation::Save)
        {
            Some("Engine settings could not be saved; retry the model selection.".to_owned())
        } else if self.catalog_controller.catalog_phase() == NativeCatalogPhase::Failed {
            Some("Runtime model catalog is unavailable. Retry model loading.".to_owned())
        } else if self.catalog_controller.favorites_failure().is_some() {
            Some("Model favorites could not be synchronized. Retry the favorite action.".to_owned())
        } else {
            None
        };
        self.model_selector.update(cx, |selector, cx| {
            if !pending_save {
                selector.set_policy(policy.clone(), cx);
            }
            selector.set_status(
                NativeModelSelectorStatus {
                    saving,
                    error,
                    authoritative,
                },
                cx,
            );
        });
    }
}

const SELECTION_SAVE_FAILED: &str =
    "Engine settings could not be saved. Your draft is preserved; retry the model selection.";

fn create_model_favorite_request_id() -> Result<RequestId, ServiceFailure> {
    mint_request_id("native-model-favorite")
}
