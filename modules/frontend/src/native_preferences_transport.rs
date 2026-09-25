//! Transport-thread child for the Forge user's preferences: the read the
//! Editor makes as it connects, the navigation it reports, and the one-time
//! import of preferences an older Editor kept in files.
//!
//! Every command is answered by exactly one [`PreferencesEvent`]; later
//! changes arrive pushed as
//! [`HostStateEvent::Preferences`](super::HostStateEvent::Preferences).

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    ImportLegacyPreferences, LegacyPreferencesImported, ReadUserPreferences, RecordNavigation,
    UserPreferences,
};

use super::*;

/// One preferences command sent from the application thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PreferencesCommand {
    /// Read the user's preferences.
    Read,
    /// Report that the user opened a project (and a thread in it).
    RecordNavigation(RecordNavigation),
    /// Hand an older Editor's file preferences to the Forge once.
    ImportLegacy(ImportLegacyPreferences),
}

/// One preferences result returned by the service child.
#[derive(Clone, Debug, PartialEq)]
pub enum PreferencesEvent {
    /// The preferences as read or as a recorded navigation left them.
    Loaded(Result<Box<UserPreferences>, ServiceFailure>),
    /// The Forge's answer to a legacy import.
    LegacyImported(Result<Box<LegacyPreferencesImported>, ServiceFailure>),
}

/// The response shape one preferences request accepts.
#[derive(Clone, Copy)]
pub(super) enum PreferencesExpectation {
    Preferences,
    LegacyImported,
}

impl PreferencesExpectation {
    /// Whether `payload` is the exact answer to this request.
    pub(super) const fn accepts(self, payload: &ResponsePayload) -> bool {
        matches!(
            (self, payload),
            (Self::Preferences, ResponsePayload::UserPreferences(_))
                | (
                    Self::LegacyImported,
                    ResponsePayload::LegacyPreferencesImported(_)
                )
        )
    }
}

/// Reads the preferences for the initial catalog load. A failed read leaves
/// the Editor on the catalog's own order rather than failing the connection.
pub(super) async fn read_preferences(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
) -> Result<Box<UserPreferences>, ServiceFailure> {
    runtime
        .request(
            frames,
            query_request(Query::ReadUserPreferences(ReadUserPreferences)),
            ExpectedResponse::Preferences(PreferencesExpectation::Preferences),
        )
        .await
        .map_err(ServiceFailure::from)
        .and_then(|payload| match payload {
            ResponsePayload::UserPreferences(preferences) => Ok(Box::new(preferences)),
            _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
        })
}

/// Handles one preferences command on the authenticated service runtime.
pub(super) async fn handle_preferences_command(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    events: &SyncSender<NativeTransportEvent>,
    command: PreferencesCommand,
) -> Result<(), ServiceFailure> {
    let event = match command {
        PreferencesCommand::Read => {
            PreferencesEvent::Loaded(read_preferences(runtime, frames).await)
        }
        PreferencesCommand::RecordNavigation(record) => {
            let request_id = record.request_id.clone();
            PreferencesEvent::Loaded(
                mutate(
                    runtime,
                    frames,
                    &request_id,
                    Command::RecordNavigation(record),
                    PreferencesExpectation::Preferences,
                )
                .await
                .and_then(|payload| match payload {
                    ResponsePayload::UserPreferences(preferences) => Ok(Box::new(preferences)),
                    _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
                }),
            )
        }
        PreferencesCommand::ImportLegacy(import) => {
            let request_id = import.request_id.clone();
            PreferencesEvent::LegacyImported(
                mutate(
                    runtime,
                    frames,
                    &request_id,
                    Command::ImportLegacyPreferences(import),
                    PreferencesExpectation::LegacyImported,
                )
                .await
                .and_then(|payload| match payload {
                    ResponsePayload::LegacyPreferencesImported(imported) => Ok(Box::new(imported)),
                    _ => Err(ServiceFailure::invalid(ServiceFailureStage::Request)),
                }),
            )
        }
    };
    publish(events, NativeTransportEvent::Preferences(event))
}

/// Sends one preferences mutation with its stable frame identity.
async fn mutate(
    runtime: &mut ServiceRuntime,
    frames: &mut FrameFactory,
    request_id: &RequestId,
    command: Command,
    expected: PreferencesExpectation,
) -> Result<ResponsePayload, ServiceFailure> {
    let frame_id = FrameId::parse(request_id.as_str().to_owned())
        .map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let sent_at =
        real_unix_millis().map_err(|_| ServiceFailure::invalid(ServiceFailureStage::Request))?;
    let mutation = StableMutation {
        frame_id,
        sent_at,
        command,
    };
    durable_save_request(
        runtime,
        frames,
        &mutation,
        ExpectedResponse::Preferences(expected),
    )
    .await
    .map_err(ServiceFailure::from)
}
