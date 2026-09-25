//! The Forge user's preferences, navigation record, and account profile.
//!
//! The Forge owns every rule: a recorded navigation moves its project to the
//! front and remembers its thread; a configuration the user saves on a
//! thread becomes the default new threads start from; a legacy import only
//! fills what the Forge does not have yet. The account profile is the host
//! account the Forge runs as.

use artisan_domain::{
    Command, EngineRunConfig, ImportLegacyPreferences, LegacyPreferencesImported, RecordNavigation,
    RequestId,
};
use artisan_protocol::{ProtocolFailure, ResponsePayload, ServerResponse};

use super::failures::{outcome, repository_failure};
use super::{RequestHandler, origin_clock_failure};

impl RequestHandler {
    /// Answers the user's preferences.
    pub(super) async fn read_user_preferences_outcome(
        &self,
        request_id: &RequestId,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let stored = self
            .repository
            .read_user_preferences()
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::UserPreferences(crate::account_profile::user_preferences(stored)),
        ))
    }

    /// Records where the user is and answers the resulting preferences.
    pub(super) async fn record_navigation_outcome(
        &self,
        request_id: &RequestId,
        record: &RecordNavigation,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let stored = self
            .repository
            .record_navigation(&record.project_id, record.thread_id.as_ref(), at)
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::UserPreferences(crate::account_profile::user_preferences(stored)),
        ))
    }

    /// Adopts an older Editor's file preferences where the Forge has none.
    /// The last-used model is resolved against the host catalog; one the
    /// catalog cannot build is refused.
    pub(super) async fn import_legacy_preferences_outcome(
        &self,
        request_id: &RequestId,
        import: &ImportLegacyPreferences,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let default = match &import.default_selection {
            None => None,
            Some(selection) => Some(
                match crate::composer_catalog_handler::host_catalog(self.account_usage.as_deref())
                    .await
                {
                    Ok(catalog) => {
                        super::model_selection::resolve_in_catalog(catalog, selection, None)
                            .ok()
                            .map(super::model_selection::ResolvedSelection::into_config)
                    }
                    Err(_) => None,
                },
            ),
        };
        let at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let imported = self
            .repository
            .import_legacy_preferences(
                default.as_ref().map(Option::as_ref),
                &import.project_order,
                at,
            )
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(outcome(
            request_id,
            ResponsePayload::LegacyPreferencesImported(LegacyPreferencesImported {
                default_model: imported.default_model,
                project_order: imported.project_order,
                preferences: crate::account_profile::user_preferences(imported.preferences),
            }),
        ))
    }

    /// Makes a configuration the user saved on a thread the default new
    /// threads start from. Best effort: the thread's own save already
    /// succeeded, and a default that could not be stored leaves the previous
    /// one.
    /// Wakes every connection to push preferences a command may have changed
    /// (a navigation, an import, or a saved configuration); only a changed
    /// value crosses the wire.
    pub(super) fn wake_preferences(&self, command: &Command) {
        if matches!(
            command,
            Command::RecordNavigation(_)
                | Command::ImportLegacyPreferences(_)
                | Command::SetThreadEngineConfig(_)
                | Command::SubmitComposerDraft(_)
        ) && let Some(notifier) = &self.conversation_commit_notifier
        {
            notifier.publish_host_state();
        }
    }

    pub(super) async fn remember_default_engine_config(&self, config: &EngineRunConfig) {
        let _ = self.repository.remember_default_engine_config(config).await;
    }
}
