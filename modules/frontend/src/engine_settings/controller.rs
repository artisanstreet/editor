//! Thread-bound engine settings controller lifecycle.
//!
//! Extracted verbatim from `engine_settings.rs` during the module split.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Application-owned controller for thread-bound engine settings.
struct PendingSave {
    thread_id: ThreadId,
    request_id: artisan_domain::RequestId,
    retained: EngineRunConfig,
}

#[derive(Clone, Copy, Default)]
struct RegistryLoadFlags {
    loading: bool,
    needed: bool,
}

#[derive(Clone, Copy, Default)]
struct SettingsLoadFlags {
    needed: bool,
    conflict_refreshing: bool,
}

pub struct EngineSettingsController {
    selected_thread: Option<ThreadId>,
    pub(super) registry: Option<RegisteredEngineProfilesResult>,
    registry_load: RegistryLoadFlags,
    settings: Option<ThreadEngineSettingsResult>,
    loading_thread: Option<ThreadId>,
    active_settings_generation: Option<SettingsLoadGeneration>,
    pub(super) next_settings_load_generation: Option<SettingsLoadGeneration>,
    settings_load: SettingsLoadFlags,
    pub(super) draft: EngineSettingsDraft,
    authoritative_revision: Option<EngineConfigRevision>,
    authoritative_config: Option<EngineRunConfig>,
    pending_save: Option<PendingSave>,
    registry_failure: Option<ServiceFailure>,
    settings_failure: Option<ServiceFailure>,
    save_failure: Option<ServiceFailure>,
    input_error: Option<EngineConfigError>,
    needs_settings_reload: Option<ThreadId>,
}

impl EngineSettingsController {
    /// Creates an empty controller with no selection.
    #[must_use]
    pub fn new() -> Self {
        Self {
            selected_thread: None,
            registry: None,
            registry_load: RegistryLoadFlags::default(),
            settings: None,
            loading_thread: None,
            active_settings_generation: None,
            next_settings_load_generation: None,
            settings_load: SettingsLoadFlags::default(),
            draft: EngineSettingsDraft::default(),
            authoritative_revision: None,
            authoritative_config: None,
            pending_save: None,
            registry_failure: None,
            settings_failure: None,
            save_failure: None,
            input_error: None,
            needs_settings_reload: None,
        }
    }

    /// Returns the selected thread.
    #[must_use]
    pub fn selected_thread(&self) -> Option<&ThreadId> {
        self.selected_thread.as_ref()
    }

    /// Returns the registry view.
    #[must_use]
    pub fn registry_view(&self) -> RegistryView {
        if self.registry_load.loading {
            return RegistryView::Loading;
        }
        match &self.registry {
            None => RegistryView::Loading,
            Some(RegisteredEngineProfilesResult::RegistryMissing) => RegistryView::Missing,
            Some(RegisteredEngineProfilesResult::RegistryPresent { profile_ids })
                if profile_ids.is_empty() =>
            {
                RegistryView::PresentEmpty
            }
            Some(RegisteredEngineProfilesResult::RegistryPresent { profile_ids }) => {
                RegistryView::Present(profile_ids.clone())
            }
        }
    }

    /// Returns the draft for UI binding.
    #[must_use]
    pub fn draft(&self) -> &EngineSettingsDraft {
        &self.draft
    }

    /// Returns a mutable draft for explicit manual editing.
    pub fn draft_mut(&mut self) -> &mut EngineSettingsDraft {
        &mut self.draft
    }

    /// Returns the authoritative settings if loaded.
    #[must_use]
    pub fn authoritative_settings(&self) -> Option<&ThreadEngineSettingsResult> {
        self.settings.as_ref()
    }

    /// Returns the exact generation accepted for the active settings load.
    #[must_use]
    pub const fn active_settings_generation(&self) -> Option<SettingsLoadGeneration> {
        self.active_settings_generation
    }

    /// Returns the exact request identity admitted for the pending save.
    #[must_use]
    pub fn pending_save_request_id(&self) -> Option<&artisan_domain::RequestId> {
        self.pending_save
            .as_ref()
            .map(|pending| &pending.request_id)
    }

    /// Returns the thread and retained configuration of the in-flight save,
    /// so a send can adopt a matching save instead of issuing a duplicate.
    #[must_use]
    pub fn pending_save(&self) -> Option<(&ThreadId, &artisan_domain::EngineRunConfig)> {
        self.pending_save
            .as_ref()
            .map(|pending| (&pending.thread_id, &pending.retained))
    }

    /// Returns the operation represented by the visible redacted failure.
    #[must_use]
    pub fn failure_operation(&self) -> Option<EngineSettingsFailureOperation> {
        if self.input_error.is_some() {
            Some(EngineSettingsFailureOperation::Input)
        } else if self.save_failure.is_some() {
            Some(EngineSettingsFailureOperation::Save)
        } else if self.settings_failure.is_some() {
            Some(EngineSettingsFailureOperation::SettingsRead)
        } else if self.registry_failure.is_some() {
            Some(EngineSettingsFailureOperation::Registry)
        } else {
            None
        }
    }

    /// Returns the finite service failure, if one is visible.
    #[must_use]
    pub fn service_failure(&self) -> Option<ServiceFailure> {
        self.save_failure
            .or(self.settings_failure)
            .or(self.registry_failure)
    }

    /// Returns the finite input error, if one is visible.
    #[must_use]
    pub const fn input_error(&self) -> Option<EngineConfigError> {
        self.input_error
    }

    /// Returns the current visible status.
    #[must_use]
    pub fn status(&self) -> EngineSettingsStatus {
        if self.selected_thread.is_none() {
            return EngineSettingsStatus::Unselected;
        }
        if self.settings_load.conflict_refreshing {
            return EngineSettingsStatus::ConflictRefreshing;
        }
        if self.pending_save.is_some() {
            return EngineSettingsStatus::Saving;
        }
        if self.failure_operation().is_some() {
            return EngineSettingsStatus::Failure;
        }
        match self.registry_view() {
            RegistryView::Missing => return EngineSettingsStatus::RegistryMissing,
            RegistryView::PresentEmpty => return EngineSettingsStatus::RegistryPresentEmpty,
            _ => {}
        }
        if self.loading_thread.is_some() || self.registry_load.loading || self.settings_load.needed
        {
            return EngineSettingsStatus::Loading;
        }
        if self.is_dirty_internal() {
            return EngineSettingsStatus::Dirty;
        }
        match &self.settings {
            Some(ThreadEngineSettingsResult::Unconfigured { .. }) => {
                EngineSettingsStatus::Unconfigured
            }
            Some(ThreadEngineSettingsResult::Configured { .. }) => EngineSettingsStatus::Ready,
            None => EngineSettingsStatus::Loading,
        }
    }

    /// Returns whether Save is enabled for the current draft.
    #[must_use]
    pub fn can_save(&self) -> bool {
        if !self.is_editable() || !self.is_dirty_internal() || self.input_error.is_some() {
            return false;
        }
        let Some(RegisteredEngineProfilesResult::RegistryPresent { .. }) = self.registry.as_ref()
        else {
            return false;
        };
        self.draft.build_config(self.registry.as_ref()).is_ok()
    }

    /// Returns whether Cancel may discard the local draft.
    #[must_use]
    pub fn can_cancel(&self) -> bool {
        self.selected_thread.is_some()
            && self.is_dirty_internal()
            && self.pending_save.is_none()
            && !self.settings_load.conflict_refreshing
            && self.loading_thread.is_none()
    }

    /// Returns the authoritative revision when configured.
    #[must_use]
    pub fn revision(&self) -> Option<EngineConfigRevision> {
        self.authoritative_revision
    }

    /// Returns the authoritative config when configured.
    #[must_use]
    pub fn authoritative_config(&self) -> Option<&EngineRunConfig> {
        self.authoritative_config.as_ref()
    }

    /// Returns whether a registry load is needed (cached for lifetime).
    #[must_use]
    pub fn needs_registry_load(&self) -> bool {
        self.registry_load.needed
    }

    /// Returns whether an authoritative settings load still needs admission.
    #[must_use]
    pub const fn needs_settings_load(&self) -> bool {
        self.settings_load.needed
    }

    /// Returns whether a conflict refresh is waiting for command admission.
    #[must_use]
    pub fn pending_reload_thread(&self) -> Option<&ThreadId> {
        self.needs_settings_reload.as_ref()
    }

    /// Marks the catalogue command admitted by the bounded bridge.
    pub fn mark_registry_load_admitted(&mut self) {
        self.registry_load.needed = false;
        self.registry_load.loading = true;
        self.registry_failure = None;
    }

    /// Retains the catalogue need after a Busy/Stopped admission refusal.
    pub fn on_registry_load_admission_failed(&mut self, failure: ServiceFailure) {
        self.registry_load.loading = false;
        self.registry_load.needed = self.selected_thread.is_some();
        self.registry_failure = Some(failure);
    }

    /// Mints the next application-owned settings-load generation.
    ///
    /// # Errors
    ///
    /// Returns a redacted request failure when no thread is selected, a load
    /// is already active, or the monotonic generation is exhausted.
    pub fn prepare_settings_load(&mut self) -> Result<SettingsLoadGeneration, ServiceFailure> {
        if self.selected_thread.is_none() || self.active_settings_generation.is_some() {
            return Err(ServiceFailure {
                stage: ServiceFailureStage::Request,
                category: ServiceFailureCategory::InvalidConfiguration,
            });
        }
        let generation = match self.next_settings_load_generation {
            None => SettingsLoadGeneration::first(),
            Some(previous) => match previous.checked_next() {
                Some(next) => next,
                None => {
                    return Err(ServiceFailure {
                        stage: ServiceFailureStage::Request,
                        category: ServiceFailureCategory::Integrity,
                    });
                }
            },
        };
        self.next_settings_load_generation = Some(generation);
        Ok(generation)
    }

    /// Commits settings-load state only after its command was admitted.
    #[must_use]
    pub fn mark_settings_load_admitted(
        &mut self,
        thread_id: &ThreadId,
        generation: SettingsLoadGeneration,
    ) -> bool {
        if self.selected_thread.as_ref() != Some(thread_id)
            || self.active_settings_generation.is_some()
            || self.next_settings_load_generation != Some(generation)
            || (self.needs_settings_reload.as_ref() != Some(thread_id)
                && !self.settings_load.needed)
        {
            return false;
        }
        self.active_settings_generation = Some(generation);
        self.loading_thread = Some(thread_id.clone());
        self.settings_load.needed = false;
        if self.needs_settings_reload.as_ref() == Some(thread_id) {
            self.needs_settings_reload = None;
        }
        self.settings_failure = None;
        true
    }

    /// Re-arms a settings read after its command was refused by the bridge.
    pub fn on_settings_load_admission_failed(
        &mut self,
        thread_id: ThreadId,
        failure: ServiceFailure,
    ) {
        if self.selected_thread.as_ref() != Some(&thread_id) {
            return;
        }
        self.active_settings_generation = None;
        self.loading_thread = None;
        if failure.category == ServiceFailureCategory::Integrity {
            // Generation exhaustion is terminal: no later command may reuse
            // an identity or pretend that an authoritative refresh is safe.
            self.settings_load.needed = false;
            self.needs_settings_reload = None;
        } else if self.settings_load.conflict_refreshing {
            self.needs_settings_reload = Some(thread_id);
        } else {
            self.settings_load.needed = true;
        }
        self.settings_failure = Some(failure);
    }

    /// Selects a real thread, clearing prior draft and stale state.
    pub fn select_thread(&mut self, thread_id: Option<&ThreadId>) {
        if self.selected_thread.as_ref() == thread_id {
            return;
        }
        self.selected_thread = thread_id.cloned();
        self.settings = None;
        self.authoritative_revision = None;
        self.authoritative_config = None;
        self.loading_thread = None;
        self.active_settings_generation = None;
        self.settings_load.needed = thread_id.is_some();
        self.draft = EngineSettingsDraft::default();
        self.pending_save = None;
        self.settings_load.conflict_refreshing = false;
        self.settings_failure = None;
        self.save_failure = None;
        self.input_error = None;
        self.needs_settings_reload = None;
        if self.registry.is_none() && !self.registry_load.loading && thread_id.is_some() {
            self.registry_load.needed = true;
        } else if self.registry.is_some() {
            self.registry_load.needed = false;
        }
    }

    /// Handles a successful registry result, preserving it for the application lifetime.
    pub fn on_registry_loaded(&mut self, result: RegisteredEngineProfilesResult) {
        self.registry = Some(result);
        self.registry_load.loading = false;
        self.registry_load.needed = false;
        self.registry_failure = None;
    }

    /// Handles a registry failure as redacted.
    pub fn on_registry_failed(&mut self, failure: ServiceFailure) {
        self.registry_load.loading = false;
        self.registry_load.needed = self.selected_thread.is_some();
        self.registry_failure = Some(failure);
    }

    /// Handles an authoritative settings result only for its exact active
    /// thread and application-minted generation.
    pub fn on_settings_loaded(
        &mut self,
        generation: SettingsLoadGeneration,
        result: ThreadEngineSettingsResult,
    ) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        if self.active_settings_generation != Some(generation)
            || self.loading_thread.as_ref() != Some(&selected)
            || result.thread_id() != &selected
        {
            return;
        }
        self.active_settings_generation = None;
        self.loading_thread = None;
        self.settings_load.needed = false;
        if self.settings_load.conflict_refreshing {
            self.settings_load.conflict_refreshing = false;
            self.needs_settings_reload = None;
        }
        self.settings_failure = None;
        self.input_error = None;
        match &result {
            ThreadEngineSettingsResult::Unconfigured { .. } => {
                self.authoritative_revision = None;
                self.authoritative_config = None;
                self.draft = EngineSettingsDraft::default();
            }
            ThreadEngineSettingsResult::Configured {
                revision, config, ..
            } => {
                self.authoritative_revision = Some(*revision);
                self.authoritative_config = Some((**config).clone());
                self.draft = EngineSettingsDraft::from_config(config);
            }
        }
        self.settings = Some(result);
    }

    /// Marks a first-send save in flight carrying a validated configuration
    /// directly. The settings draft stays `OpenCode` 2-shaped until
    /// per-engine settings UI lands, so native selections cannot travel
    /// through [`Self::can_save`]; the retained configuration converges the
    /// draft on [`Self::on_save_succeeded`] instead.
    #[must_use]
    pub fn begin_direct_save(
        &mut self,
        thread_id: ThreadId,
        request_id: artisan_domain::RequestId,
        retained: EngineRunConfig,
    ) -> bool {
        if self.selected_thread.as_ref() != Some(&thread_id) || self.pending_save.is_some() {
            return false;
        }
        self.pending_save = Some(PendingSave {
            thread_id,
            request_id,
            retained,
        });
        self.save_failure = None;
        self.input_error = None;
        true
    }

    /// Marks a save in flight only after the exact command was admitted.
    #[must_use]
    pub fn begin_saving(
        &mut self,
        pending_thread: ThreadId,
        request_id: artisan_domain::RequestId,
        retained: EngineRunConfig,
    ) -> bool {
        if self.selected_thread.as_ref() != Some(&pending_thread)
            || self.pending_save.is_some()
            || !self.can_save()
        {
            return false;
        }
        self.pending_save = Some(PendingSave {
            thread_id: pending_thread,
            request_id,
            retained,
        });
        self.save_failure = None;
        self.input_error = None;
        true
    }

    /// Records a Busy/Stopped save admission refusal without losing the draft.
    pub fn on_save_admission_failed(&mut self, failure: ServiceFailure) {
        self.pending_save = None;
        if self.selected_thread.is_some() {
            self.save_failure = Some(failure);
        }
    }

    /// Handles a successful save, storing returned revision plus retained config.
    pub fn on_save_succeeded(
        &mut self,
        result: &SetThreadEngineConfigResult,
        retained: EngineRunConfig,
    ) {
        let Some(selected) = self.selected_thread.as_ref() else {
            return;
        };
        let Some(pending) = self.pending_save.as_ref() else {
            return;
        };
        if result.thread_id.as_str() != selected.as_str()
            || result.thread_id.as_str() != pending.thread_id.as_str()
            || result.request_id.as_str() != pending.request_id.as_str()
            || pending.retained != retained
        {
            return;
        }
        self.authoritative_revision = Some(result.revision);
        self.authoritative_config = Some(retained.clone());
        self.draft = EngineSettingsDraft::from_config(&retained);
        self.settings = Some(ThreadEngineSettingsResult::Configured {
            thread_id: selected.clone(),
            revision: result.revision,
            config: Box::new(retained),
        });
        self.pending_save = None;
        self.save_failure = None;
        self.input_error = None;
        self.settings_load.conflict_refreshing = false;
        self.needs_settings_reload = None;
    }

    /// Handles a conflict event only for the exact pending save request.
    pub fn on_conflict(&mut self, thread_id: ThreadId, request_id: &artisan_domain::RequestId) {
        let Some(selected) = self.selected_thread.as_ref() else {
            return;
        };
        if thread_id.as_str() != selected.as_str() {
            return;
        }
        if self.pending_save.as_ref().is_none_or(|pending| {
            pending.thread_id.as_str() != thread_id.as_str()
                || pending.request_id.as_str() != request_id.as_str()
        }) {
            return;
        }
        self.pending_save = None;
        self.active_settings_generation = None;
        self.loading_thread = None;
        self.settings_load.conflict_refreshing = true;
        self.needs_settings_reload = Some(thread_id);
        self.save_failure = None;
    }

    /// Handles a redacted save failure for the exact pending request.
    pub fn on_save_failed(
        &mut self,
        thread_id: &ThreadId,
        request_id: &artisan_domain::RequestId,
        failure: ServiceFailure,
    ) {
        let Some(selected) = self.selected_thread.as_ref() else {
            return;
        };
        if thread_id.as_str() != selected.as_str() {
            return;
        }
        if self.pending_save.as_ref().is_none_or(|pending| {
            pending.thread_id.as_str() != thread_id.as_str()
                || pending.request_id.as_str() != request_id.as_str()
        }) {
            return;
        }
        self.pending_save = None;
        self.save_failure = Some(failure);
    }

    /// Handles a settings-read failure for the exact active generation.
    pub fn on_settings_load_failed(
        &mut self,
        thread_id: ThreadId,
        generation: SettingsLoadGeneration,
        failure: ServiceFailure,
    ) {
        let Some(selected) = self.selected_thread.clone() else {
            return;
        };
        if thread_id != selected
            || self.active_settings_generation != Some(generation)
            || self.loading_thread.as_ref() != Some(&thread_id)
        {
            return;
        }
        self.active_settings_generation = None;
        self.loading_thread = None;
        if self.settings_load.conflict_refreshing {
            self.needs_settings_reload = Some(thread_id);
        } else {
            self.settings_load.needed = false;
        }
        self.settings_failure = Some(failure);
    }

    /// Cancels local edits without emitting a save.
    pub fn cancel(&mut self) {
        if !self.can_cancel() {
            return;
        }
        if let Some(config) = self.authoritative_config.clone() {
            self.draft = EngineSettingsDraft::from_config(&config);
        } else {
            self.draft = EngineSettingsDraft::default();
        }
        self.input_error = None;
        self.save_failure = None;
    }

    fn is_dirty_internal(&self) -> bool {
        let base = if let Some(config) = &self.authoritative_config {
            EngineSettingsDraft::from_config(config)
        } else {
            EngineSettingsDraft::default()
        };
        self.draft != base
    }

    /// Returns whether draft differs from authoritative.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.is_dirty_internal()
    }

    fn is_editable(&self) -> bool {
        self.selected_thread.is_some()
            && self.settings.is_some()
            && self.loading_thread.is_none()
            && self.active_settings_generation.is_none()
            && self.pending_save.is_none()
            && !self.settings_load.conflict_refreshing
    }

    /// Selects one profile only when it came from the authoritative registry.
    #[must_use]
    pub fn select_profile(&mut self, profile_id: &EngineProfileId) -> bool {
        let present = matches!(
            self.registry.as_ref(),
            Some(RegisteredEngineProfilesResult::RegistryPresent { profile_ids })
                if profile_ids.iter().any(|id| id == profile_id)
        );
        if !present || !self.is_editable() {
            self.input_error = Some(EngineConfigError::new(
                "profile_id",
                EngineConfigReason::InvalidIdentifier,
            ));
            return false;
        }
        profile_id.as_str().clone_into(&mut self.draft.profile_id);
        self.input_error = None;
        true
    }

    /// Applies a complete, strictly structured clipboard configuration.
    ///
    /// # Errors
    ///
    /// Returns only a redacted field/reason pair; the source document is
    /// never stored or formatted.
    pub fn apply_manual_configuration(&mut self, document: &str) -> Result<(), EngineConfigError> {
        if !self.is_editable() {
            let error = EngineConfigError::new("configuration", EngineConfigReason::Unsupported);
            self.input_error = Some(error);
            return Err(error);
        }
        let parsed = match parse_manual_configuration(document) {
            Ok(draft) => draft,
            Err(error) => {
                self.input_error = Some(error);
                return Err(error);
            }
        };
        self.draft = parsed;
        self.input_error = None;
        self.save_failure = None;
        Ok(())
    }

    /// Builds the pending `SetThreadEngineConfig` when Save is enabled.
    ///
    /// Caller must supply a fresh `RequestId` minted for this save. The
    /// transport will retain it across one reconnect retry.
    #[must_use]
    pub fn build_save_command(
        &self,
        request_id: artisan_domain::RequestId,
    ) -> Option<artisan_domain::SetThreadEngineConfig> {
        if !self.can_save() {
            return None;
        }
        let thread_id = self.selected_thread.clone()?;
        let config = self.draft.build_config(self.registry.as_ref()).ok()?;
        let precondition = self.authoritative_revision.map_or_else(
            || artisan_domain::EngineConfigUpdatePrecondition::Unconfigured,
            artisan_domain::EngineConfigUpdatePrecondition::Exact,
        );
        Some(artisan_domain::SetThreadEngineConfig::new(
            request_id,
            thread_id,
            precondition,
            config,
        ))
    }

    /// Builds a pending `SetThreadEngineConfig` for an already validated
    /// engine configuration, bypassing the `OpenCode` 2-shaped draft.
    ///
    /// Caller must supply a fresh `RequestId` minted for this save and a
    /// configuration built by [`crate::composer_model_config::config_for_policy`].
    /// The compare-and-swap precondition is shared with
    /// [`Self::build_save_command`]: `Unconfigured` while no authoritative
    /// revision exists (first send), otherwise `Exact` on the authoritative
    /// revision so a concurrent writer conflicts instead of being silently
    /// overwritten. Returns [`None`] while no thread is selected, a save is
    /// already in flight, or a conflict refresh is outstanding.
    #[must_use]
    pub fn build_direct_save_command(
        &self,
        request_id: artisan_domain::RequestId,
        config: artisan_domain::EngineRunConfig,
    ) -> Option<artisan_domain::SetThreadEngineConfig> {
        if self.selected_thread.is_none()
            || self.pending_save.is_some()
            || self.settings_load.conflict_refreshing
        {
            return None;
        }
        let thread_id = self.selected_thread.clone()?;
        let precondition = self.authoritative_revision.map_or_else(
            || artisan_domain::EngineConfigUpdatePrecondition::Unconfigured,
            artisan_domain::EngineConfigUpdatePrecondition::Exact,
        );
        Some(artisan_domain::SetThreadEngineConfig::new(
            request_id,
            thread_id,
            precondition,
            config,
        ))
    }
}

impl Default for EngineSettingsController {
    fn default() -> Self {
        Self::new()
    }
}
