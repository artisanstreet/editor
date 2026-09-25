use super::*;
use crate::native_model_catalog::NativeOptionValue;
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
                // The retry behind every composer error refreshes the
                // backend-probed account verdict first, so a recovered
                // sign-in or repaired binary is observed instead of
                // re-failing on a stale row.
                self.ensure_profile_usage(false, None, cx);
                if let Some(pending) = self.catalog_controller.pending_favorite().cloned() {
                    if !pending.admitted {
                        self.submit_pending_model_favorite(&pending, cx);
                    }
                } else if self.engine_settings.can_save() {
                    self.save_engine_settings(cx);
                } else if !self.retry_native_policy_save(cx) {
                    self.refresh_composer_block_reason(cx);
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
        // Native choices without an explicit profile persist under the
        // supported default profile, so the stored choice, the save, and the
        // send-time check all observe the same durable identity.
        let policy = crate::composer_model_config::with_default_native_profile(policy);
        // Every committed choice refreshes the last-used preference, so new
        // threads without saved config start from this model.
        if let Some(stored) = crate::native_last_used::save_model_policy(&policy) {
            self.last_used_model = Some(stored);
        }
        self.composer_model_choice = Some((self.selected_thread.clone(), policy.clone()));
        self.composer_model_run_error = None;
        self.sync_composer_controls(cx);
        let Some(thread) = self.selected_thread.clone() else {
            return;
        };
        if self.engine_settings.pending_save_request_id().is_some() {
            self.deferred_composer_policy = Some((thread, policy));
            return;
        }
        self.deferred_composer_policy = None;
        let catalog = self.effective_catalog_snapshot(cx);
        let outcome = crate::composer_model_config::config_for_policy(
            &catalog,
            &policy,
            self.engine_settings.authoritative_config(),
        );
        match outcome {
            Ok(config) => {
                if self.engine_settings.authoritative_config() == Some(&config) {
                    self.sync_composer_model_policy(cx);
                    return;
                }
                if matches!(
                    config.selection(),
                    artisan_domain::EngineSelection::OpenCode2(_)
                ) {
                    *self.engine_settings.draft_mut() =
                        crate::engine_settings::EngineSettingsDraft::from_config(&config);
                    if self.engine_settings.can_save() {
                        self.save_engine_settings(cx);
                    }
                } else if let Some(thread_id) = self.selected_thread.clone() {
                    // Native selections share the direct typed-save path with
                    // first sends (compare-and-swap `Unconfigured`/`Exact`
                    // from the controller) instead of the `OpenCode` 2-shaped
                    // draft, so a later model/effort/profile change on a
                    // configured thread saves exactly like the first one.
                    if !self.submit_direct_save(thread_id, config) {
                        self.composer_model_run_error = Some(
                            "Engine settings could not be saved. Your draft is preserved; retry the model selection."
                                .to_owned(),
                        );
                    }
                }
                self.sync_composer_model_policy(cx);
            }
            // The choice stays local until its engine can persist a run config.
            // Report configuration failure only if the user tries to send.
            Err(_) => {
                self.sync_composer_model_policy(cx);
            }
        }
    }

    /// Retries a native model/effort/profile choice through the shared direct
    /// typed-save path.
    ///
    /// Returns whether a native save was attempted: the displayed choice for
    /// the selected thread rebuilds against the readiness-overlaid catalog
    /// and submits with the controller-derived precondition, so the retry
    /// behind a composer error recovers a stranded selection exactly like
    /// the original one. `OpenCode` 2 choices stay on the draft path and
    /// report `false`.
    fn retry_native_policy_save(&mut self, cx: &mut Context<Self>) -> bool {
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
        let catalog = self.effective_catalog_snapshot(cx);
        let Ok(config) = crate::composer_model_config::config_for_policy(
            &catalog,
            &choice,
            self.engine_settings.authoritative_config(),
        ) else {
            return false;
        };
        if matches!(
            config.selection(),
            artisan_domain::EngineSelection::OpenCode2(_)
        ) {
            return false;
        }
        if !self.submit_direct_save(thread_id, config) {
            self.composer_model_run_error = Some(
                "Engine settings could not be saved. Your draft is preserved; retry the model selection."
                    .to_owned(),
            );
        }
        self.sync_composer_model_policy(cx);
        cx.notify();
        true
    }

    /// Refreshes the composer block message from the current probed account
    /// verdict.
    ///
    /// Used when the model retry finds no pending favorite, draft, or native
    /// save to submit: the message names the latest readiness state (which
    /// the same retry just asked to refresh) instead of stranding the stale
    /// send-time copy. Configured threads and in-flight saves keep their
    /// owning flows' messages.
    fn refresh_composer_block_reason(&mut self, cx: &mut Context<Self>) {
        if self.engine_settings.authoritative_config().is_some()
            || self.engine_settings.pending_save_request_id().is_some()
        {
            return;
        }
        let displayed = match &self.composer_model_choice {
            Some((thread, policy)) if thread == &self.selected_thread => Some(policy.clone()),
            _ => self.model_selector.read(cx).state().policy().cloned(),
        };
        let Some(policy) = displayed else {
            return;
        };
        self.composer_model_run_error = Some(self.readiness_block_reason(&policy.engine_id));
        self.sync_composer_controls(cx);
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

    /// Seeds the displayed policy from the last-used preference.
    ///
    /// Only for threads with no per-thread choice and no saved
    /// configuration: saved threads never reach this fallback, and the
    /// result stays non-authoritative until the thread saves its own.
    fn last_used_display_policy(
        &self,
        snapshot: &crate::native_model_catalog::NativeModelCatalog,
    ) -> Option<crate::native_model_catalog::NativeModelPolicy> {
        let stored = self.last_used_model.as_ref()?;
        crate::native_last_used::restore_policy(snapshot, stored)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one policy sync resolves the per-thread choice, saved config, last-used fallback, rebase, authority, and selector status in display order"
    )]
    pub(super) fn sync_composer_model_policy(&mut self, cx: &mut Context<Self>) {
        if self
            .composer_model_choice
            .as_ref()
            .is_some_and(|(thread, _)| thread != &self.selected_thread)
        {
            self.composer_model_choice = None;
            self.composer_model_run_error = None;
        }
        // Admission and matching observe the readiness-overlaid catalog, so a
        // probed ambient account reads as authoritative without a managed
        // registry while a signed-out engine never does.
        let snapshot = self.effective_catalog_snapshot(cx);
        let policy = self
            .engine_settings
            .authoritative_config()
            .and_then(|config| {
                // A saved native selection projects back through the shared
                // config boundary, so a reloaded or switched-back thread
                // displays its saved model/effort/profile/permission instead
                // of drifting to no selection. Only an exact round-trip is
                // accepted; anything else falls back to no saved policy.
                let artisan_domain::EngineSelection::OpenCode2(native) = config.selection() else {
                    return crate::composer_model_config::policy_for_selection(&snapshot, config)
                        .ok();
                };
                if snapshot.scope.as_ref()?.profile_id != native.profile_id().as_str() {
                    return None;
                }
                let model = snapshot.manifest.models.iter().find(|model| {
                    model.native_selection.as_ref().is_some_and(|selection| {
                        selection.model_id == native.model_id().as_str()
                            && selection.provider_route_id == native.route_id().as_str()
                            && selection.variant_id.as_deref()
                                == native
                                    .variant_id()
                                    .map(artisan_domain::EngineVariantId::as_str)
                    })
                })?;
                let mut policy = snapshot.preview_policy_for_model(&model.id).ok()?;
                let option = snapshot
                    .manifest
                    .harness(&policy.engine_id)?
                    .permissions
                    .options
                    .iter()
                    .find(|option| option.id == native.permission().permission_id().as_str())?;
                policy.permission = Some(NativeOptionValue {
                    id: option.id.clone(),
                    native_value: option.native_value.clone(),
                });
                Some(policy)
            });
        let saved_policy = policy;
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
            .or_else(|| self.last_used_display_policy(&snapshot));
        // The saved-policy projection above stays `OpenCode` 2-shaped;
        // another engine contributes no saved policy yet. For a native
        // authoritative configuration the displayed choice is authoritative
        // exactly when it rebuilds to the saved run configuration — the
        // same equality the send gate enforces — so a saved Codex/Claude
        // selection reads as saved instead of permanently drifting.
        let authoritative = match (&policy, &saved_policy) {
            (Some(displayed), Some(saved)) => displayed == saved,
            (Some(displayed), None) => {
                self.engine_settings
                    .authoritative_config()
                    .is_some_and(|saved| {
                        crate::composer_model_config::config_for_policy(
                            &snapshot,
                            displayed,
                            Some(saved),
                        )
                        .ok()
                        .as_ref()
                            == Some(saved)
                    })
            }
            _ => false,
        };
        let pending_save = self.engine_settings.pending_save_request_id().is_some();
        let saving = pending_save
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

static MODEL_FAVORITE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn create_model_favorite_request_id() -> Result<RequestId, ServiceFailure> {
    let process_id = u64::from(std::process::id());
    let millis = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| invalid_service_failure())?
            .as_millis(),
    )
    .map_err(|_| invalid_service_failure())?;
    let counter = MODEL_FAVORITE_COUNTER
        .fetch_update(
            std::sync::atomic::Ordering::Relaxed,
            std::sync::atomic::Ordering::Relaxed,
            |current| current.checked_add(1),
        )
        .map_err(|_| invalid_service_failure())?;
    RequestId::parse(format!(
        "native-model-favorite-{process_id}-{millis}-{counter}"
    ))
    .map_err(|_| invalid_service_failure())
}
