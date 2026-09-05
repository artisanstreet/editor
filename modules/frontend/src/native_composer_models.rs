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
            NativeModelSelectorEvent::Retry => {
                if let Some(pending) = self.catalog_controller.pending_favorite().cloned() {
                    if !pending.admitted { self.submit_pending_model_favorite(pending, cx); }
                } else if self.engine_settings.can_save() {
                    self.save_engine_settings(cx);
                } else {
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
    }

    fn handle_composer_policy_selection(
        &mut self,
        policy: &crate::native_model_selector::SelectPolicy,
        cx: &mut Context<Self>,
    ) {
        self.composer_model_choice = Some((self.selected_thread.clone(), policy.clone()));
        self.composer_model_run_error = None;
        self.sync_composer_controls(cx);
        if self.selected_thread.is_none()
            || self.engine_settings.pending_save_request_id().is_some()
        {
            return;
        }
        let catalog = self.model_selector.read(cx).state().snapshot().clone();
        let outcome = crate::composer_model_config::config_for_policy(
            &catalog,
            policy,
            self.engine_settings.authoritative_config(),
        );
        match outcome {
            Ok(config) => {
                *self.engine_settings.draft_mut() =
                    crate::engine_settings::EngineSettingsDraft::from_config(&config);
                if self.engine_settings.can_save() {
                    self.save_engine_settings(cx);
                    self.sync_composer_model_policy(cx);
                } else {
                    self.sync_composer_model_policy(cx);
                }
            }
            // The choice stays local until its engine can persist a run config.
            // Report configuration failure only if the user tries to send.
            Err(_) => {},
        }
    }

    fn handle_composer_favorite(&mut self, intent: &SetFavorite, cx: &mut Context<Self>) {
        if let Some(pending) = self.catalog_controller.pending_favorite().cloned() {
            if pending.model_id.as_str() == intent.model_id && pending.favorite == intent.favorite {
                if !pending.admitted {
                    self.submit_pending_model_favorite(pending, cx);
                } else {
                    self.sync_composer_catalog_status(cx);
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
        let model_id = match ModelFavoriteId::parse(intent.model_id.clone()) {
            Ok(model_id) => model_id,
            Err(_) => {
                self.set_model_selector_error("The selected model identity is invalid.", cx);
                return;
            }
        };
        let catalog_revision = match CatalogRevision::parse(catalog.catalog_revision.clone()) {
            Ok(revision) => revision,
            Err(_) => {
                self.set_model_selector_error(
                    "The runtime catalog revision is invalid; reload the catalog before retrying.",
                    cx,
                );
                return;
            }
        };
        let request_id = match create_model_favorite_request_id() {
            Ok(request_id) => request_id,
            Err(_) => {
                self.set_model_selector_error("Favorite request identity is unavailable.", cx);
                return;
            }
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
        self.submit_pending_model_favorite(pending, cx);
    }

    pub(super) fn submit_pending_model_favorite(
        &mut self,
        pending: crate::native_transport::PendingModelFavorite,
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
            self.catalog_controller.on_favorite_admission_failed(
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
                    self.catalog_controller.on_favorite_admission_failed(
                        &pending.scope,
                        &pending.request_id,
                        invalid_service_failure(),
                    );
                }
            }
            Err(error) => {
                self.catalog_controller.on_favorite_admission_failed(
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

    pub(super) fn sync_composer_model_policy(&mut self, cx: &mut Context<Self>) {
        if self.composer_model_choice.as_ref().is_some_and(|(thread, _)| thread != &self.selected_thread) {
            self.composer_model_choice = None;
            self.composer_model_run_error = None;
        }
        let snapshot = self.model_selector.read(cx).state().snapshot();
        let policy = self
            .engine_settings
            .authoritative_config()
            .and_then(|config| {
                let native = config.selection().as_opencode2();
                if snapshot.scope.as_ref()?.profile_id != native.profile_id().as_str() {
                    return None;
                }
                let model = snapshot.manifest.models.iter().find(|model| {
                    model.native_selection.as_ref().is_some_and(|selection| {
                        selection.model_id == native.model_id().as_str()
                            && selection.provider_route_id == native.route_id().as_str()
                            && selection.variant_id.as_deref()
                                == native.variant_id().map(|v| v.as_str())
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
        let policy = self.composer_model_choice.as_ref()
            .and_then(|(_, choice)| snapshot.rebase_policy(choice))
            .or_else(|| saved_policy.clone());
        let authoritative = policy.is_some() && policy == saved_policy;
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
