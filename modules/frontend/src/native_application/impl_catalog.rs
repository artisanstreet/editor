//! Catalog discovery, favorite synchronization, and model status for
//! [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-4 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    /// Refreshes the active scope or the local home picker every five minutes.
    pub(super) fn refresh_model_catalog(
        &mut self,
        catalog: Option<NativeModelCatalog>,
        cx: &mut Context<Self>,
    ) {
        if self.shutdown_prepared || self.service_stopped {
            return;
        }
        if self.selected_thread.is_some() {
            if self.catalog_controller.pending_favorite().is_none() {
                if let Some(scope) = self.catalog_controller.refresh_catalog() {
                    self.submit_composer_catalog_reads(&scope, cx);
                } else if self.catalog_controller.scope().is_none() {
                    self.discover_composer_catalog_for_settings(cx);
                }
            }
        } else if self.machine_home.is_none()
            && let Some(catalog) = catalog
            && self
                .model_selector
                .read(cx)
                .state()
                .snapshot()
                .catalog_revision
                != catalog.catalog_revision
        {
            self.host_model_catalog = Some(catalog);
            self.reset_model_selector_offline(cx);
            self.sync_composer_model_policy(cx);
            cx.notify();
        }
    }

    pub(super) fn reset_model_selector_offline(&mut self, cx: &mut Context<Self>) {
        // The Forge publishes a scope-free catalog snapshot (static baseline
        // plus live discovery) for surfaces without a thread-scoped runtime
        // read; fall back to the bundled manifest when it is absent.
        let catalog = self
            .host_model_catalog
            .clone()
            .or_else(|| {
                self.machine_home
                    .is_none()
                    .then(scope_free_catalog_snapshot)
                    .flatten()
            })
            .unwrap_or_else(|| {
                NativeModelCatalog::offline()
                    .expect("the bundled model catalog is validated at the native boundary")
            });
        self.model_selector.update(cx, |selector, cx| {
            selector.set_snapshot(catalog, cx);
            selector.set_policy(None, cx);
            selector.set_status(NativeModelSelectorStatus::default(), cx);
        });
    }

    /// Returns the selector snapshot overlaid with backend-probed account
    /// readiness.
    ///
    /// Static native models whose engine carries a fresh authenticated usage
    /// report are admittable without any managed `OpenCode` profile or
    /// registry; the overlay recomputes that gated subset on every call so a
    /// signed-out, failed, or stale engine never inherits a previous
    /// admission from the stored snapshot.
    pub(super) fn effective_catalog_snapshot(&self, cx: &App) -> NativeModelCatalog {
        let snapshot = self.model_selector.read(cx).state().snapshot().clone();
        catalog_with_usage_readiness(snapshot, &self.profile_usage, profile_usage_now_ms())
    }

    /// Returns the actionable reason a displayed policy's engine cannot run.
    ///
    /// The verdict comes from the backend-probed usage row, never from a
    /// catch-all, and the wording is shared across engines: a signed-out
    /// engine names sign-in, a pending read says a check is running, and any
    /// other unavailable state reports the check status plus the actual
    /// probed failure when one exists. Nothing here claims a missing
    /// installation or broken binary without executable evidence â€” a stale,
    /// failed, or never-probed check reads as a status problem with a
    /// refresh recovery, since the installed authenticated account is the
    /// established baseline. Every message preserves the draft and points
    /// at the working recovery (the model retry that refreshes account
    /// status, or Settings â†’ Engines). A `Ready` engine that the catalog
    /// still rejects is a catalog-side unavailability, not an account
    /// problem.
    pub(super) fn readiness_block_reason(&self, engine_id: &str) -> String {
        let label = profile_usage_display_name(engine_id);
        match engine_readiness(&self.profile_usage, engine_id, profile_usage_now_ms()) {
            EngineReadiness::Ready => "This model is unavailable in the runtime catalog right now. Your draft is preserved; retry or pick another model.".to_owned(),
            EngineReadiness::NeedsSignIn => format!(
                "{label} account sign-in is required. Your draft is preserved; open Settings â†’ Engines â†’ {label} to review it, or retry to refresh."
            ),
            EngineReadiness::Checking => format!(
                "Checking the {label} account status. Your draft is preserved; retry in a moment."
            ),
            EngineReadiness::NotReady => {
                match engine_refresh_failure(&self.profile_usage, engine_id) {
                    Some(failure) => format!(
                        "{label} account status is unavailable: {failure}. Your draft is preserved; retry to refresh, or open Settings â†’ Engines â†’ {label}."
                    ),
                    None => format!(
                        "{label} account status is unavailable right now. Your draft is preserved; retry to refresh its status, or open Settings â†’ Engines â†’ {label}."
                    ),
                }
            }
        }
    }

    pub(super) fn reset_composer_catalog(&mut self, cx: &mut Context<Self>) {
        self.catalog_controller.clear_scope();
        self.reset_model_selector_offline(cx);
    }

    pub(super) fn discover_composer_catalog(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        cx: &mut Context<Self>,
    ) {
        let selection = match self.catalog_controller.select_scope(thread_id, profile_id) {
            Ok(selection) => selection,
            Err(CatalogScopeError::GenerationExhausted) => {
                self.model_selector.update(cx, |selector, cx| {
                    selector.set_status(
                        NativeModelSelectorStatus {
                            saving: false,
                            error: Some(
                                "Runtime model catalog loading is unavailable for this session."
                                    .to_owned(),
                            ),
                            authoritative: false,
                        },
                        cx,
                    );
                });
                return;
            }
        };
        if selection.changed() {
            self.reset_model_selector_offline(cx);
        }
        self.submit_composer_catalog_reads(selection.scope(), cx);
    }

    pub(super) fn submit_composer_catalog_reads(
        &mut self,
        scope: &NativeCatalogScope,
        cx: &mut Context<Self>,
    ) {
        if self.catalog_controller.catalog_request_needed(scope) {
            let command = NativeTransportCommand::ReadComposerCatalog {
                thread_id: scope.thread_id.clone(),
                profile_id: scope.profile_id.clone(),
                generation: scope.generation,
            };
            let outcome = self.submit_command(command);
            match outcome {
                Ok(()) => {
                    if !self.catalog_controller.mark_catalog_admitted(scope) {
                        self.catalog_controller
                            .on_catalog_admission_failed(scope, invalid_service_failure());
                    }
                }
                Err(error) => {
                    self.catalog_controller
                        .on_catalog_admission_failed(scope, command_failure(error));
                }
            }
        }
        if self.catalog_controller.favorites_request_needed(scope) {
            let command = NativeTransportCommand::ReadModelFavorites {
                thread_id: scope.thread_id.clone(),
                profile_id: scope.profile_id.clone(),
                generation: scope.generation,
            };
            let outcome = self.submit_command(command);
            match outcome {
                Ok(()) => {
                    if !self.catalog_controller.mark_favorites_admitted(scope) {
                        self.catalog_controller
                            .on_favorites_admission_failed(scope, invalid_service_failure());
                    }
                }
                Err(error) => {
                    self.catalog_controller
                        .on_favorites_admission_failed(scope, command_failure(error));
                }
            }
        }
        self.sync_composer_catalog_status(cx);
    }

    /// Retries the current catalog read without minting a new scope.
    pub fn retry_composer_catalog(&mut self, cx: &mut Context<Self>) {
        if let Some(scope) = self.catalog_controller.retry_catalog() {
            self.submit_composer_catalog_reads(&scope, cx);
        }
    }

    /// Returns whether the current runtime catalog exposes an explicit retry.
    #[must_use]
    pub fn composer_catalog_retry_available(&self) -> bool {
        self.catalog_controller.catalog_retry_available()
    }

    /// Returns the truthful lifecycle of the selected runtime catalog.
    #[must_use]
    pub fn composer_catalog_phase(&self) -> NativeCatalogPhase {
        self.catalog_controller.catalog_phase()
    }

    /// Retries the current durable favorites read without minting a new scope.
    pub fn retry_composer_favorites(&mut self, cx: &mut Context<Self>) {
        if let Some(scope) = self.catalog_controller.retry_favorites() {
            self.submit_composer_catalog_reads(&scope, cx);
        }
    }

    /// Returns whether the current durable favorites read exposes an explicit
    /// retry.
    #[must_use]
    pub fn composer_favorites_retry_available(&self) -> bool {
        self.catalog_controller.favorites_retry_available()
    }

    /// Retries the exact favorite mutation retained after a failure.
    pub fn retry_composer_favorite(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = self.catalog_controller.retry_favorite() {
            self.submit_pending_model_favorite(&pending, cx);
        }
    }

    /// Returns whether an exact favorite mutation may be retried.
    #[must_use]
    pub fn composer_favorite_retry_available(&self) -> bool {
        self.catalog_controller.retry_favorite().is_some()
    }

    pub(super) fn discover_composer_catalog_for_settings(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        let profile = self
            .engine_settings
            .authoritative_config()
            .map(|config| config.selection().profile_id().clone())
            .or_else(|| match self.engine_settings.registry_view() {
                crate::engine_settings::RegistryView::Present(profiles) if profiles.len() == 1 => {
                    profiles.into_iter().next()
                }
                _ => None,
            });
        let profile_id = profile.unwrap_or_else(|| {
            EngineProfileId::parse(crate::composer_model_config::NATIVE_DEFAULT_PROFILE_ID)
                .expect("native default profile is valid")
        });
        self.discover_composer_catalog(thread_id, profile_id, cx);
        self.sync_composer_model_policy(cx);
        self.ensure_profile_usage(false, None, cx);
    }

    pub(super) fn sync_composer_catalog_status(&mut self, cx: &mut Context<Self>) {
        let error = if self.catalog_controller.catalog_phase() == NativeCatalogPhase::Failed {
            Some("Runtime model catalog is unavailable. Retry model loading.".to_owned())
        } else if self.catalog_controller.favorites_failure().is_some() {
            Some("Model favorites could not be synchronized. Retry the favorite action.".to_owned())
        } else {
            None
        };
        let saving = self.catalog_controller.catalog_loading()
            || self
                .catalog_controller
                .pending_favorite()
                .is_some_and(|pending| pending.admitted);
        let authoritative = self.catalog_controller.catalog_ready();
        self.model_selector.update(cx, |selector, cx| {
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

    #[expect(
        clippy::needless_pass_by_value,
        reason = "the decoded event's small id newtypes are moved into the handler and only read; borrowing from the match arm would not avoid the field access"
    )]
    pub(super) fn handle_composer_catalog(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
        result: &artisan_protocol::ComposerCatalogResult,
        cx: &mut Context<Self>,
    ) {
        let scope = NativeCatalogScope::new(thread_id.clone(), profile_id.clone(), generation);
        if !self.catalog_controller.catalog_response_current(&scope)
            || result.thread_id != thread_id
            || result.profile_id != profile_id
        {
            return;
        }
        let Ok(mut catalog) = result.snapshot.decoded() else {
            let _ = self.catalog_controller.on_catalog_failed(
                &scope,
                ServiceFailure {
                    stage: ServiceFailureStage::Request,
                    category: ServiceFailureCategory::Integrity,
                },
            );
            self.sync_composer_catalog_status(cx);
            return;
        };
        if catalog
            .scope
            .as_ref()
            .is_none_or(|catalog_scope| catalog_scope.profile_id.as_str() != profile_id.as_str())
        {
            let _ = self.catalog_controller.on_catalog_failed(
                &scope,
                ServiceFailure {
                    stage: ServiceFailureStage::Request,
                    category: ServiceFailureCategory::Integrity,
                },
            );
            self.sync_composer_catalog_status(cx);
            return;
        }
        if !self.catalog_controller.on_catalog_loaded(&scope) {
            return;
        }
        if self.catalog_controller.favorite_revision().is_some() {
            catalog.favorite_ids = self.catalog_controller.favorite_ids().to_vec();
        }
        // Keep all discovered models visible while the next conversation loads.
        // The cache is presentation only for OpenCode: discard scoped admission
        // and defaults, but retain model and route metadata for the picker.
        let mut host_catalog = catalog.clone();
        host_catalog.scope = None;
        host_catalog
            .runnable_harness_ids
            .retain(|engine| engine != "opencode2");
        host_catalog.default_model_id = None;
        host_catalog.model_defaults.clear();
        self.host_model_catalog = Some(host_catalog);
        self.model_selector.update(cx, |selector, cx| {
            selector.set_snapshot(catalog, cx);
        });
        self.sync_composer_model_policy(cx);
        self.sync_composer_catalog_status(cx);
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    pub(super) fn handle_composer_catalog_failed(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let scope = NativeCatalogScope::new(thread_id, profile_id, generation);
        if self.catalog_controller.on_catalog_failed(&scope, failure) {
            self.sync_composer_catalog_status(cx);
            self.refresh_settings_engine_snapshot(cx);
            cx.notify();
        }
    }

    pub(super) fn handle_model_favorites(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
        result: &artisan_protocol::ModelFavoritesSnapshot,
        cx: &mut Context<Self>,
    ) {
        let scope = NativeCatalogScope::new(thread_id, profile_id, generation);
        if !self.catalog_controller.favorites_response_current(&scope) {
            return;
        }
        let revision = result.revision.get();
        let model_ids = result
            .model_ids
            .iter()
            .map(|model_id| model_id.as_str().to_owned())
            .collect::<Vec<_>>();
        let applied = self
            .catalog_controller
            .on_favorites_loaded(&scope, revision, model_ids);
        if applied {
            self.apply_authoritative_favorites(cx);
        }
        self.sync_composer_catalog_status(cx);
        cx.notify();
    }

    pub(super) fn handle_model_favorites_failed(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        generation: CatalogLoadGeneration,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let scope = NativeCatalogScope::new(thread_id, profile_id, generation);
        if self.catalog_controller.on_favorites_failed(&scope, failure) {
            self.sync_composer_catalog_status(cx);
            cx.notify();
        }
    }

    pub(super) fn apply_authoritative_favorites(&mut self, cx: &mut Context<Self>) {
        let favorite_ids = self.catalog_controller.favorite_ids().to_vec();
        let mut catalog = self.model_selector.read(cx).state().snapshot().clone();
        catalog.favorite_ids = favorite_ids;
        self.model_selector.update(cx, |selector, cx| {
            selector.set_snapshot(catalog, cx);
        });
        self.sync_composer_model_policy(cx);
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "the decoded event's small id newtypes are moved into the handler and only read; borrowing from the match arm would not avoid the field access"
    )]
    pub(super) fn handle_model_favorite_set(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        request_id: RequestId,
        receipt: &artisan_protocol::SetModelFavoriteReceipt,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.catalog_controller.scope().cloned() else {
            return;
        };
        if scope.thread_id != thread_id
            || scope.profile_id != profile_id
            || receipt.request_id != request_id
        {
            return;
        }
        let model_id = receipt.model_id.clone();
        let favorite = receipt.favorite;
        let revision = receipt.snapshot.revision.get();
        let model_ids = receipt
            .snapshot
            .model_ids
            .iter()
            .map(|model_id| model_id.as_str().to_owned())
            .collect::<Vec<_>>();
        if self.catalog_controller.on_favorite_succeeded(
            &scope,
            &request_id,
            &model_id,
            favorite,
            revision,
            model_ids,
        ) {
            self.apply_authoritative_favorites(cx);
            self.sync_composer_catalog_status(cx);
            cx.notify();
        }
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "the decoded event's small id newtypes are moved into the handler and only read; borrowing from the match arm would not avoid the field access"
    )]
    pub(super) fn handle_model_favorite_failed(
        &mut self,
        thread_id: ThreadId,
        profile_id: EngineProfileId,
        request_id: RequestId,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let Some(scope) = self.catalog_controller.scope().cloned() else {
            return;
        };
        if scope.thread_id != thread_id || scope.profile_id != profile_id {
            return;
        }
        if self
            .catalog_controller
            .on_favorite_failed(&scope, &request_id, failure)
        {
            self.sync_composer_catalog_status(cx);
            cx.notify();
        }
    }
}
