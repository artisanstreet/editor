//! The Forge user's preferences as the Editor renders them.
//!
//! The Forge keeps the default engine configuration, the project order,
//! the thread last open in each project, the last route, and the account
//! the Forge runs as. The Editor reads them as it connects, orders its
//! projects and resumes its route from them, reports where the user goes,
//! and shows the default model on threads without their own configuration.
//! Within a session the order and remembered threads follow the user's own
//! navigation (the Forge applies the same most-recently-used rule to the
//! reports); pushes and answers refresh only the default model and the
//! account. Preferences an older Editor kept in files are handed to the
//! Forge once and then removed.

use artisan_domain::{
    ImportLegacyPreferences, NavigationRecord, NavigationRoute, RecordNavigation, UserPreferences,
};

use crate::editor_settings::LegacyForgePreferences;
use crate::native_transport_service::{PreferencesCommand, PreferencesEvent};

use super::*;

impl impl_projects::ProjectNavigation {
    /// Adopts the navigation record read as the Editor connected: the
    /// project order and route this connection resumes, and the thread last
    /// open in each project. Returns whether this was the first record.
    pub(super) fn adopt_record(&mut self, record: &NavigationRecord) -> bool {
        if self.forge_order.is_some() {
            return false;
        }
        self.forge_order = Some(
            record
                .projects()
                .iter()
                .map(|entry| entry.project_id.clone())
                .collect(),
        );
        for entry in record.projects() {
            if let Some(thread) = &entry.last_thread_id {
                self.last_threads
                    .entry(entry.project_id.clone())
                    .or_insert_with(|| thread.clone());
            }
        }
        self.route = record.route().cloned();
        true
    }

    /// The project of the route the connection resumes, if one was recorded.
    pub(super) fn resumed_project(&self) -> Option<&ProjectId> {
        self.route
            .as_ref()
            .map(|route: &NavigationRoute| &route.project_id)
    }
}

impl NativeApplication {
    /// Applies one preferences answer.
    pub(super) fn handle_preferences_event(
        &mut self,
        event: PreferencesEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            PreferencesEvent::Loaded(Ok(preferences)) => {
                if self
                    .project_navigation
                    .adopt_record(&preferences.navigation)
                {
                    self.import_legacy_preferences(&preferences);
                }
                self.apply_forge_preferences(&preferences, cx);
            }
            // Without the Forge's record the catalog order stands; later
            // navigation is still reported.
            PreferencesEvent::Loaded(Err(_)) => {}
            PreferencesEvent::LegacyImported(Ok(imported)) => {
                if let Some(legacy) = self.legacy_import.take() {
                    legacy.retire();
                }
                self.apply_forge_preferences(&imported.preferences, cx);
            }
            // The files stay for the next connection.
            PreferencesEvent::LegacyImported(Err(_)) => self.legacy_import = None,
        }
    }

    /// Shows the account the Forge runs as and the default model it keeps.
    pub(super) fn apply_forge_preferences(
        &mut self,
        preferences: &UserPreferences,
        cx: &mut Context<Self>,
    ) {
        let account = &preferences.account;
        self.profile_name = Some(account.display_name.as_str().to_owned());
        self.profile_hostname = Some(account.host_name.as_str().to_owned());
        cx.set_global(crate::native_account_identity::ArtisanAccountIdentity {
            display_name: account.display_name.as_str().to_owned(),
        });
        if self.default_engine_config != preferences.default_engine_config {
            self.default_engine_config
                .clone_from(&preferences.default_engine_config);
            self.sync_composer_model_policy(cx);
            self.refresh_settings_engine_snapshot(cx);
        }
        cx.notify();
    }

    /// Reports that the user opened `project`, and `thread` in it, unless it
    /// is what was last reported. Best effort: a report the connection
    /// refuses is simply not made, and the next navigation reports again.
    pub(super) fn report_navigation(&mut self, project: ProjectId, thread: Option<ThreadId>) {
        let report = (project, thread);
        if self.project_navigation.reported.as_ref() == Some(&report) {
            return;
        }
        let Ok(request_id) = mint_request_id("native-navigation") else {
            return;
        };
        let (project_id, thread_id) = report.clone();
        let command = NativeTransportCommand::Preferences(PreferencesCommand::RecordNavigation(
            RecordNavigation {
                request_id,
                project_id,
                thread_id,
            },
        ));
        if self.submit_command(command).is_ok() {
            self.project_navigation.reported = Some(report);
        }
    }

    /// Hands preferences an older Editor kept in files to the Forge: the
    /// last-used model when the Forge has no default yet, and the project
    /// order when it has no navigation record yet. The files are removed
    /// once the Forge answered, or at once when it needs neither.
    fn import_legacy_preferences(&mut self, preferences: &UserPreferences) {
        let legacy = self.legacy_forge_preferences();
        if legacy.is_empty() {
            return;
        }
        let default_selection = legacy
            .selection
            .clone()
            .filter(|_| preferences.default_engine_config.is_none());
        let project_order = if preferences.navigation.projects().is_empty() {
            legacy.project_order.clone()
        } else {
            Vec::new()
        };
        if default_selection.is_none() && project_order.is_empty() {
            legacy.retire();
            return;
        }
        let Ok(request_id) = mint_request_id("native-legacy-preferences") else {
            return;
        };
        let command = NativeTransportCommand::Preferences(PreferencesCommand::ImportLegacy(
            ImportLegacyPreferences {
                request_id,
                default_selection,
                project_order,
            },
        ));
        if self.submit_command(command).is_ok() {
            self.legacy_import = Some(legacy);
        }
    }

    #[cfg(not(test))]
    fn legacy_forge_preferences(&mut self) -> LegacyForgePreferences {
        crate::editor_settings::legacy_forge_preferences(self.machine_home.as_deref())
    }

    #[cfg(test)]
    fn legacy_forge_preferences(&mut self) -> LegacyForgePreferences {
        self.test_legacy_preferences.take().unwrap_or_default()
    }
}
