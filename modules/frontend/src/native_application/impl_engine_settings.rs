//! Engine-settings controller flows for [`NativeApplication`]: registry loads, model selection, save/cancel, and clipboard template handling.
//!
//! Extracted verbatim from `native_application.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

#[expect(
    dead_code,
    reason = "engine-settings profile/save/cancel/clipboard handlers and controller accessors are the typed entry points for the engine-settings surface; that wiring is owned by the frontend-native-engine-settings lane"
)]
impl NativeApplication {
    pub(super) fn request_engine_settings_for_selected(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        // Thread selection owns the readiness refresh alongside the settings
        // and registry reads: the composer gate below evaluates the probed
        // verdict, so selection must request it rather than inheriting
        // whatever the profile popover last loaded.
        self.ensure_profile_usage(false, None, cx);
        if self.engine_settings.needs_registry_load() {
            self.submit_registry_load();
        }
        if self.engine_settings.needs_settings_load()
            || self.engine_settings.pending_reload_thread().is_some()
        {
            self.submit_settings_load(thread_id);
        }
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    pub(super) fn submit_registry_load(&mut self) {
        if !self.engine_settings.needs_registry_load() {
            return;
        }
        // Registry and settings loads travel the shared submission
        // boundary so the mock sink observes the same commands production
        // sends; a refused bridge reports identically either way.
        match self.submit_command(NativeTransportCommand::ListRegisteredProfiles) {
            Ok(()) => self.engine_settings.mark_registry_load_admitted(),
            Err(error) => self
                .engine_settings
                .on_registry_load_admission_failed(command_failure(error)),
        }
    }

    pub(super) fn submit_settings_load(&mut self, thread_id: ThreadId) {
        let generation = match self.engine_settings.prepare_settings_load() {
            Ok(generation) => generation,
            Err(failure) => {
                self.engine_settings
                    .on_settings_load_admission_failed(thread_id, failure);
                return;
            }
        };
        let command = NativeTransportCommand::LoadThreadEngineSettings {
            thread_id: thread_id.clone(),
            generation,
        };
        // Same shared boundary as the registry load above: the mock sink
        // admits the load in tests while a stopped bridge fails closed.
        match self.submit_command(command) {
            Ok(()) => {
                if !self
                    .engine_settings
                    .mark_settings_load_admitted(&thread_id, generation)
                {
                    self.engine_settings.on_settings_load_admission_failed(
                        thread_id,
                        ServiceFailure {
                            stage: ServiceFailureStage::Request,
                            category: ServiceFailureCategory::Integrity,
                        },
                    );
                }
            }
            Err(error) => self
                .engine_settings
                .on_settings_load_admission_failed(thread_id, command_failure(error)),
        }
    }

    pub(super) fn handle_engine_settings(
        &mut self,
        generation: SettingsLoadGeneration,
        result: artisan_protocol::ThreadEngineSettingsResult,
        cx: &mut Context<Self>,
    ) {
        let accepted = self.engine_settings.active_settings_generation() == Some(generation)
            && self
                .selected_thread
                .as_ref()
                .is_some_and(|thread_id| result.thread_id() == thread_id);
        self.engine_settings.on_settings_loaded(generation, result);
        self.sync_composer_model_policy(cx);
        if accepted {
            self.discover_composer_catalog_for_settings(cx);
        }
        self.refresh_settings_engine_snapshot(cx);
        self.sync_composer_availability(cx);
        cx.notify();
    }

    pub(super) fn handle_registered_profiles(
        &mut self,
        result: artisan_protocol::RegisteredEngineProfilesResult,
        cx: &mut Context<Self>,
    ) {
        self.engine_settings.on_registry_loaded(result);
        self.discover_composer_catalog_for_settings(cx);
        cx.notify();
    }

    pub(super) fn handle_registered_profiles_failed(
        &mut self,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        self.engine_settings.on_registry_failed(failure);
        cx.notify();
    }

    /// Continues the authoritative save confirmation for a proactive
    /// selection-time save. Sends are never held for this acknowledgment:
    /// it only seats the durable configuration that later sends resolve
    /// their engine (and steer naming) against.
    pub(super) fn handle_engine_config_set(
        &mut self,
        result: &artisan_protocol::SetThreadEngineConfigResult,
        retained: artisan_domain::EngineRunConfig,
        cx: &mut Context<Self>,
    ) {
        let accepted = self.engine_settings.selected_thread() == Some(&result.thread_id)
            && self.engine_settings.pending_save_request_id() == Some(&result.request_id);
        self.engine_settings.on_save_succeeded(result, retained);
        self.sync_composer_model_policy(cx);
        if accepted {
            self.discover_composer_catalog_for_settings(cx);
        }
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    pub(super) fn handle_engine_conflict(
        &mut self,
        thread_id: ThreadId,
        request_id: &artisan_domain::RequestId,
        cx: &mut Context<Self>,
    ) {
        let accepted = self.engine_settings.selected_thread() == Some(&thread_id)
            && self.engine_settings.pending_save_request_id() == Some(request_id);
        self.engine_settings.on_conflict(thread_id, request_id);
        if accepted && self.engine_settings.pending_reload_thread().is_some() {
            self.reset_composer_catalog(cx);
        }
        if self.engine_settings.pending_reload_thread().is_some() {
            self.request_engine_settings_for_selected(cx);
        } else {
            self.refresh_settings_engine_snapshot(cx);
            cx.notify();
        }
    }

    pub(super) fn handle_engine_config_failed(
        &mut self,
        thread_id: &ThreadId,
        request_id: &artisan_domain::RequestId,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        self.engine_settings
            .on_save_failed(thread_id, request_id, failure);
        self.sync_composer_model_policy(cx);
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    pub(super) fn handle_engine_settings_failed(
        &mut self,
        thread_id: ThreadId,
        generation: SettingsLoadGeneration,
        failure: ServiceFailure,
        cx: &mut Context<Self>,
    ) {
        let accepted = self.engine_settings.active_settings_generation() == Some(generation)
            && self.selected_thread.as_ref() == Some(&thread_id);
        self.engine_settings
            .on_settings_load_failed(thread_id, generation, failure);
        if accepted {
            self.reset_composer_catalog(cx);
        }
        self.refresh_settings_engine_snapshot(cx);
        cx.notify();
    }

    /// Returns the displayed model policy for one engine, if the composer
    /// choice or the selector policy names it.
    ///
    /// The explicit choice wins over the selector default; the returned
    /// policy carries the default native profile so Settings saves observe
    /// the same durable identity as composer saves.
    pub(super) fn displayed_policy_for_engine(
        &self,
        engine_id: &str,
        cx: &App,
    ) -> Option<crate::native_model_selector::SelectPolicy> {
        if let Some((thread, choice)) = self.composer_model_choice.as_ref()
            && thread == &self.selected_thread
            && choice.engine_id == engine_id
        {
            return Some(choice.clone());
        }
        let policy = self.model_selector.read(cx).state().policy().cloned()?;
        (policy.engine_id == engine_id).then_some(
            crate::composer_model_config::with_default_native_profile(&policy),
        )
    }

    /// Builds the live engine snapshot for one engine settings page.
    ///
    /// Every row is projected from current application state â€” the
    /// readiness-overlaid catalog, the probed usage rows, the managed
    /// registry view, and the thread configuration â€” so the page paints
    /// loaded, loading, unavailable, and sign-in states without inventing
    /// installation facts.
    #[expect(
        clippy::too_many_lines,
        reason = "one projection pass keeps the engine settings snapshot fields in review order"
    )]
    pub(super) fn settings_engine_snapshot(
        &self,
        engine_id: &str,
        cx: &App,
    ) -> SettingsEngineSnapshot {
        let now_ms = profile_usage_now_ms();
        let readiness = engine_readiness(&self.profile_usage, engine_id, now_ms);
        let entry = self.profile_usage.entry(engine_id);
        let account_email = entry.as_ref().and_then(|entry| {
            entry
                .report
                .as_ref()
                .and_then(|report| report.account_email.clone())
        });
        let refresh_failure = engine_refresh_failure(&self.profile_usage, engine_id);
        let refreshing = self
            .profile_usage
            .refreshing_engine_ids
            .iter()
            .any(|refreshing| refreshing == engine_id);
        let phase = self.catalog_controller.catalog_phase();
        let catalog = match phase {
            NativeCatalogPhase::Ready => SettingsEngineCatalogState::Ready,
            NativeCatalogPhase::Failed => SettingsEngineCatalogState::Failed,
            NativeCatalogPhase::Loading | NativeCatalogPhase::Offline => {
                SettingsEngineCatalogState::Loading
            }
        };
        let catalog_error = if phase == NativeCatalogPhase::Failed {
            Some("Runtime model catalog is unavailable. Retry model loading.".to_owned())
        } else if self.catalog_controller.favorites_failure().is_some() {
            Some("Model favorites could not be synchronized. Retry the favorite action.".to_owned())
        } else {
            None
        };
        let registry = match self.engine_settings.registry_view() {
            RegistryView::Loading => SettingsEngineRegistryState::Loading,
            RegistryView::Missing => SettingsEngineRegistryState::Missing,
            RegistryView::PresentEmpty => SettingsEngineRegistryState::Empty,
            RegistryView::Present(_) => SettingsEngineRegistryState::Present,
        };
        let selected_thread = self
            .selected_thread
            .as_ref()
            .map(|thread| thread.as_str().to_owned());
        let authoritative = self.engine_settings.authoritative_config();
        let effective = self.effective_catalog_snapshot(cx);
        let saved_policy = authoritative.and_then(|config| {
            if config.selection().engine_id().as_str() != engine_id {
                return None;
            }
            crate::composer_model_config::policy_for_selection(&effective, config).ok()
        });
        let saved_model = saved_policy
            .as_ref()
            .map(|policy| policy.model_id.clone())
            .or_else(|| {
                authoritative.and_then(|config| {
                    (config.selection().engine_id().as_str() == engine_id)
                        .then(|| {
                            config
                                .selection()
                                .model_id()
                                .map(|model| model.as_str().to_owned())
                        })
                        .flatten()
                })
            });
        let saved_profile = authoritative.and_then(|config| {
            (config.selection().engine_id().as_str() == engine_id)
                .then(|| config.selection().profile_id().as_str().to_owned())
        });
        let displayed = self.displayed_policy_for_engine(engine_id, cx);
        let displayed_model = displayed.as_ref().map(|policy| policy.model_id.clone());
        let displayed_authoritative = match (&displayed, authoritative) {
            (Some(displayed), Some(saved)) => {
                crate::composer_model_config::config_for_policy(&effective, displayed, Some(saved))
                    .ok()
                    .as_ref()
                    == Some(saved)
            }
            _ => false,
        };
        let pending_save = self.engine_settings.pending_save_request_id().is_some();
        let save_failed =
            self.engine_settings.failure_operation() == Some(EngineSettingsFailureOperation::Save);
        let can_save_displayed = selected_thread.is_some()
            && !pending_save
            && !displayed_authoritative
            && displayed.as_ref().is_some_and(|policy| {
                crate::composer_model_config::config_for_policy(&effective, policy, authoritative)
                    .is_ok_and(|config| Some(&config) != authoritative)
            });
        // An explicit choice held without a save names its honest blocker:
        // no thread, or the live admission reason. Saved, saving, and
        // failed states read out their own rows instead.
        let choice_notice = match (&displayed, &selected_thread) {
            (Some(policy), None)
                if self
                    .composer_model_choice
                    .as_ref()
                    .is_some_and(|(thread, choice)| {
                        thread == &self.selected_thread && choice.engine_id == engine_id
                    }) =>
            {
                Some(format!(
                    "â€œ{}â€ is selected. Select a thread to save it.",
                    policy.model_id
                ))
            }
            (Some(policy), Some(_))
                if !pending_save
                    && !save_failed
                    && !displayed_authoritative
                    && self
                        .composer_model_choice
                        .as_ref()
                        .is_some_and(|(thread, choice)| {
                            thread == &self.selected_thread && choice.engine_id == engine_id
                        })
                    && effective.admit_policy(policy).is_err() =>
            {
                Some(self.readiness_block_reason(&policy.engine_id))
            }
            _ => None,
        };
        let saved_id = saved_policy.as_ref().map(|policy| policy.model_id.clone());
        let models = effective
            .manifest
            .models
            .iter()
            .filter(|model| model.harness == engine_id)
            .map(|model| SettingsEngineModel {
                id: model.id.clone(),
                saved: saved_id.as_deref() == Some(model.id.as_str()),
                displayed: displayed_model.as_deref() == Some(model.id.as_str()),
                disabled_reason: model
                    .disabled
                    .as_ref()
                    .map(|disabled| disabled.reason.clone()),
            })
            .collect();
        SettingsEngineSnapshot {
            engine_id: engine_id.to_owned(),
            readiness,
            account_email,
            refresh_failure,
            refreshing,
            catalog,
            catalog_error,
            registry,
            selected_thread,
            saved_model,
            saved_profile,
            displayed_model,
            displayed_authoritative,
            can_save_displayed,
            pending_save,
            save_failed,
            choice_notice,
            models,
        }
    }

    /// Rebuilds the mounted engine snapshot, if an engine page is mounted.
    ///
    /// Called from transport and settings event handlers â€” never from
    /// render-synced projections â€” so the page follows acknowledgments,
    /// conflicts, failures, catalog reads, and usage replies.
    pub(super) fn refresh_settings_engine_snapshot(&mut self, cx: &mut Context<Self>) {
        let (Some(screen), Some((SettingsRoute::Engines, Some(engine_id)))) = (
            self.settings_screen.clone(),
            self.settings_screen_key.clone(),
        ) else {
            return;
        };
        if engine_id == crate::native_settings::FIXTURE_ENGINE_ID {
            return;
        }
        let snapshot = self.settings_engine_snapshot(&engine_id, cx);
        screen.update(cx, |screen, screen_cx| {
            screen.set_engine_snapshot(snapshot, screen_cx);
        });
    }

    /// Serves one Settings model choice through the shared picker flow.
    ///
    /// The catalog model becomes a `SelectPolicy` on the effective catalog
    /// and travels the existing composer selection path â€” same admission,
    /// same direct typed save with compare-and-swap, same acknowledgment â€”
    /// so a Settings choice is durable exactly like a composer one. An
    /// engine mismatch or unknown model is ignored; an unrunnable choice is
    /// stored with the live admission reason.
    pub(super) fn choose_settings_engine_model(
        &mut self,
        engine_id: &str,
        model_id: &str,
        cx: &mut Context<Self>,
    ) {
        let catalog = self.effective_catalog_snapshot(cx);
        let Ok(raw) = catalog.selection_policy_for_model(model_id) else {
            return;
        };
        if raw.engine_id != engine_id {
            return;
        }
        let policy = crate::composer_model_config::with_default_native_profile(&raw);
        if catalog.admit_policy(&policy).is_err() {
            self.composer_model_run_error = Some(self.readiness_block_reason(&policy.engine_id));
        }
        self.handle_composer_model_event(
            &crate::native_model_selector::NativeModelSelectorEvent::SelectPolicy(policy),
            cx,
        );
        self.refresh_settings_engine_snapshot(cx);
    }

    /// Saves the Settings-displayed model through the shared direct save.
    pub(super) fn save_settings_displayed_model(
        &mut self,
        engine_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        if self.engine_settings.pending_save_request_id().is_some() {
            return;
        }
        let Some(policy) = self.displayed_policy_for_engine(engine_id, cx) else {
            return;
        };
        let catalog = self.effective_catalog_snapshot(cx);
        let Ok(config) = crate::composer_model_config::config_for_policy(
            &catalog,
            &policy,
            self.engine_settings.authoritative_config(),
        ) else {
            self.composer_model_run_error = Some(self.readiness_block_reason(&policy.engine_id));
            self.sync_composer_controls(cx);
            return;
        };
        if Some(&config) == self.engine_settings.authoritative_config() {
            return;
        }
        if !self.submit_direct_save(thread_id, config) {
            self.composer_model_run_error = Some(
                "Engine settings could not be saved. Your draft is preserved; retry the model selection."
                    .to_owned(),
            );
        }
        self.sync_composer_model_policy(cx);
        cx.notify();
    }

    /// Serves one mounted Settings screen action.
    pub(super) fn handle_settings_screen_event(
        &mut self,
        _screen: Entity<SettingsScreen>,
        event: &SettingsScreenEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            SettingsScreenEvent::Navigate { section, engine } => {
                self.navigate(
                    NativeRoute::Settings {
                        section: *section,
                        engine: engine.clone(),
                    },
                    cx,
                );
            }
            SettingsScreenEvent::RefreshEngine { engine_id } => {
                self.ensure_profile_usage(true, Some(engine_id), cx);
                self.refresh_settings_engine_snapshot(cx);
            }
            SettingsScreenEvent::SaveDisplayedModel { engine_id } => {
                self.save_settings_displayed_model(engine_id, cx);
                self.refresh_settings_engine_snapshot(cx);
            }
            SettingsScreenEvent::SelectEngineModel {
                engine_id,
                model_id,
            } => {
                self.choose_settings_engine_model(engine_id, model_id, cx);
            }
        }
    }

    pub(super) fn select_engine_profile(
        &mut self,
        profile_id: &EngineProfileId,
        cx: &mut Context<Self>,
    ) {
        if self.engine_settings.select_profile(profile_id) {
            cx.notify();
        }
    }

    pub(super) fn copy_manual_configuration_template(cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(manual_configuration_template()));
        cx.notify();
    }

    pub(super) fn paste_manual_configuration_from_clipboard(&mut self, cx: &mut Context<Self>) {
        let document = cx.read_from_clipboard().and_then(|item| item.text());
        if let Some(document) = document {
            let _ = self.engine_settings.apply_manual_configuration(&document);
        } else {
            // Route an absent/non-text clipboard through the same bounded
            // parser path without retaining or displaying clipboard data.
            let _ = self.engine_settings.apply_manual_configuration("");
        }
        cx.notify();
    }

    pub(super) fn handle_copy_manual_configuration(
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        Self::copy_manual_configuration_template(cx);
    }

    pub(super) fn handle_paste_manual_configuration(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.paste_manual_configuration_from_clipboard(cx);
    }

    pub(super) fn handle_select_engine_profile(
        &mut self,
        profile_id: &EngineProfileId,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_engine_profile(profile_id, cx);
    }

    pub(super) fn handle_save_engine_settings(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_engine_settings(cx);
    }

    /// Submits a validated engine configuration through the shared direct
    /// typed-save path, bypassing the `OpenCode` 2-shaped draft.
    ///
    /// The compare-and-swap precondition comes from the controller:
    /// `Unconfigured` for a first send, `Exact` on the authoritative
    /// revision for a later model/effort/profile change, so a concurrent
    /// writer conflicts instead of being silently overwritten. The save is
    /// tracked for its real acknowledgment, which seats the authoritative
    /// configuration; sends are never held for it. Returns whether the save
    /// is now tracked for its authoritative acknowledgment.
    pub(super) fn submit_direct_save(
        &mut self,
        thread_id: ThreadId,
        config: artisan_domain::EngineRunConfig,
    ) -> bool {
        // The controller selection follows the mounted thread; selecting
        // again is a no-op when it already does.
        self.engine_settings.select_thread(Some(&thread_id));
        let request_id = match create_save_request_id() {
            Ok(id) => id,
            Err(failure) => {
                self.engine_settings.on_save_admission_failed(failure);
                return false;
            }
        };
        let Some(command) = self
            .engine_settings
            .build_direct_save_command(request_id.clone(), config.clone())
        else {
            return false;
        };
        if !self
            .engine_settings
            .begin_direct_save(thread_id, request_id, config)
        {
            return false;
        }
        match self.submit_command(NativeTransportCommand::SetThreadEngineConfig(Box::new(
            command,
        ))) {
            Ok(()) => true,
            Err(error) => {
                self.engine_settings
                    .on_save_admission_failed(command_failure(error));
                false
            }
        }
    }

    pub(super) fn handle_cancel_engine_settings(
        &mut self,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_engine_settings(cx);
    }

    /// Attempts to save the current draft when valid and visible.
    pub(super) fn save_engine_settings(&mut self, cx: &mut Context<Self>) {
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        if !self.engine_settings.can_save() {
            return;
        }
        let request_id = match create_save_request_id() {
            Ok(id) => id,
            Err(failure) => {
                self.engine_settings.on_save_admission_failed(failure);
                self.sync_composer_model_policy(cx);
                cx.notify();
                return;
            }
        };
        let Some(command) = self.engine_settings.build_save_command(request_id) else {
            self.engine_settings
                .on_save_admission_failed(ServiceFailure {
                    stage: ServiceFailureStage::Request,
                    category: ServiceFailureCategory::InvalidConfiguration,
                });
            self.sync_composer_model_policy(cx);
            cx.notify();
            return;
        };
        let request_id = command.request_id().clone();
        let retained_config = command.config().clone();
        let Some(service) = self.service.clone() else {
            self.engine_settings
                .on_save_admission_failed(ServiceFailure {
                    stage: ServiceFailureStage::EventBridge,
                    category: ServiceFailureCategory::ChannelClosed,
                });
            self.sync_composer_model_policy(cx);
            cx.notify();
            return;
        };
        match service.submit(NativeTransportCommand::SetThreadEngineConfig(Box::new(
            command,
        ))) {
            Ok(()) => {
                if !self
                    .engine_settings
                    .begin_saving(thread_id, request_id, retained_config)
                {
                    self.engine_settings
                        .on_save_admission_failed(ServiceFailure {
                            stage: ServiceFailureStage::Request,
                            category: ServiceFailureCategory::Integrity,
                        });
                }
            }
            Err(error) => self
                .engine_settings
                .on_save_admission_failed(command_failure(error)),
        }
        self.sync_composer_model_policy(cx);
        cx.notify();
    }

    /// Cancels local edits without emitting a save.
    pub(super) fn cancel_engine_settings(&mut self, cx: &mut Context<Self>) {
        self.engine_settings.cancel();
        self.sync_composer_model_policy(cx);
        cx.notify();
    }

    /// Returns the engine-settings controller for inspection.
    #[must_use]
    pub(super) fn engine_settings(&self) -> &EngineSettingsController {
        &self.engine_settings
    }

    /// Returns a mutable reference to the engine-settings controller.
    pub(super) fn engine_settings_mut(&mut self) -> &mut EngineSettingsController {
        &mut self.engine_settings
    }
}
