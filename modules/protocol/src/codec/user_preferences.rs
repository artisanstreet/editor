//! Parent-union dispatch for the Forge user's preferences, navigation
//! record, and account profile (stateless Editor step 7).

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::{
    AccountProfile, ImportLegacyPreferences, LegacyImportOutcome, LegacyPreferencesImported,
    NAVIGATION_PROJECTS_MAX, NavigationProject, NavigationRecord, NavigationRoute,
    ReadUserPreferences, RecordNavigation, UserPreferences,
};

use crate::composer_state_codec as leaf;

/// Encodes one preferences request arm.
pub(crate) fn encode_user_preferences_request(
    mut builder: request::Builder<'_>,
    value: &ClientRequest,
) -> Result<(), ProtocolEncodeError> {
    match value {
        ClientRequest::Query(Query::ReadUserPreferences(_)) => {
            builder.set_read_user_preferences(());
        }
        ClientRequest::Command(Command::RecordNavigation(record)) => {
            let mut encoded = builder.init_record_navigation();
            encoded.set_project_id(record.project_id.as_str());
            encoded.set_thread_id(record.thread_id.as_ref().map_or("", ThreadId::as_str));
        }
        ClientRequest::Command(Command::ImportLegacyPreferences(import)) => {
            let mut encoded = builder.init_import_legacy_preferences();
            if let Some(selection) = &import.default_selection {
                leaf::encode_catalog_selection(
                    encoded.reborrow().init_default_selection(),
                    selection,
                );
            }
            let mut order = encoded.init_project_order(list_length(
                "request.importLegacyPreferences.projectOrder",
                import.project_order.len(),
            )?);
            for (index, project) in import.project_order.iter().enumerate() {
                order.set(
                    list_index("request.importLegacyPreferences.projectOrder", index)?,
                    project.as_str(),
                );
            }
        }
        _ => return Err(ProtocolEncodeError::ComposerState),
    }
    Ok(())
}

/// Decodes one preferences request arm.
pub(crate) fn decode_user_preferences_request(
    value: request::Reader<'_>,
    request_id: &RequestId,
) -> Result<ClientRequest, ProtocolDecodeError> {
    match value.which()? {
        request::Which::ReadUserPreferences(()) => Ok(ClientRequest::Query(
            Query::ReadUserPreferences(ReadUserPreferences),
        )),
        request::Which::RecordNavigation(record) => {
            let record = record?;
            Ok(ClientRequest::Command(Command::RecordNavigation(
                RecordNavigation {
                    request_id: request_id.clone(),
                    project_id: parse_project_id(
                        read_text(
                            record.get_project_id(),
                            "request.recordNavigation.projectId",
                        )?,
                        "request.recordNavigation.projectId",
                    )?,
                    thread_id: optional_thread(
                        record.get_thread_id(),
                        "request.recordNavigation.threadId",
                    )?,
                },
            )))
        }
        request::Which::ImportLegacyPreferences(import) => {
            let import = import?;
            let field = "request.importLegacyPreferences.projectOrder";
            let order = import.get_project_order()?;
            if order.len() as usize > NAVIGATION_PROJECTS_MAX {
                return Err(leaf::ComposerStateCodecError::StateValue { field }.into());
            }
            let project_order = order
                .iter()
                .map(|project| parse_project_id(read_text(project, field)?, field))
                .collect::<Result<Vec<_>, _>>()?;
            let default_selection = if import.has_default_selection() {
                Some(leaf::decode_catalog_selection(
                    import.get_default_selection()?,
                    "request.importLegacyPreferences.defaultSelection",
                )?)
            } else {
                None
            };
            Ok(ClientRequest::Command(Command::ImportLegacyPreferences(
                ImportLegacyPreferences {
                    request_id: request_id.clone(),
                    default_selection,
                    project_order,
                },
            )))
        }
        _ => Err(leaf::ComposerStateCodecError::StateValue {
            field: "request.userPreferences",
        }
        .into()),
    }
}

/// Encodes one preferences response arm.
pub(crate) fn encode_user_preferences_response(
    mut builder: response::Builder<'_>,
    payload: &ResponsePayload,
) -> Result<(), ProtocolEncodeError> {
    match payload {
        ResponsePayload::UserPreferences(preferences) => {
            encode_preferences(builder.reborrow().init_user_preferences(), preferences)
        }
        ResponsePayload::LegacyPreferencesImported(imported) => {
            let mut encoded = builder.reborrow().init_legacy_preferences_imported();
            encoded.set_default_model(encode_outcome(imported.default_model));
            encoded.set_project_order(encode_outcome(imported.project_order));
            encode_preferences(encoded.init_preferences(), &imported.preferences)
        }
        _ => Err(ProtocolEncodeError::ComposerState),
    }
}

/// Decodes one preferences response arm.
pub(crate) fn decode_user_preferences_response(
    value: response::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    match value.which()? {
        response::Which::UserPreferences(preferences) => Ok(ResponsePayload::UserPreferences(
            decode_preferences(preferences?)?,
        )),
        response::Which::LegacyPreferencesImported(imported) => {
            let imported = imported?;
            Ok(ResponsePayload::LegacyPreferencesImported(
                LegacyPreferencesImported {
                    default_model: decode_outcome(imported.get_default_model()?),
                    project_order: decode_outcome(imported.get_project_order()?),
                    preferences: decode_preferences(imported.get_preferences()?)?,
                },
            ))
        }
        _ => Err(leaf::ComposerStateCodecError::StateValue {
            field: "response.userPreferences",
        }
        .into()),
    }
}

/// Encodes the preferences value shared by both answers.
pub(crate) fn encode_preferences(
    mut builder: artisan_capnp::user_preferences::Builder<'_>,
    value: &UserPreferences,
) -> Result<(), ProtocolEncodeError> {
    builder.set_revision(value.revision);
    if let Some(config) = &value.default_engine_config {
        encode_engine_run_config(builder.reborrow().init_default_engine_config(), config);
    }
    let projects = value.navigation.projects();
    let mut encoded = builder
        .reborrow()
        .init_projects(list_length("userPreferences.projects", projects.len())?);
    for (index, project) in projects.iter().enumerate() {
        let mut entry = encoded
            .reborrow()
            .get(list_index("userPreferences.projects", index)?);
        entry.set_project_id(project.project_id.as_str());
        entry.set_last_thread_id(project.last_thread_id.as_ref().map_or("", ThreadId::as_str));
    }
    if let Some(route) = value.navigation.route() {
        let mut encoded = builder.reborrow().init_route();
        encoded.set_project_id(route.project_id.as_str());
        encoded.set_thread_id(route.thread_id.as_ref().map_or("", ThreadId::as_str));
    }
    let mut account = builder.init_account();
    account.set_display_name(value.account.display_name.as_str());
    account.set_host_name(value.account.host_name.as_str());
    Ok(())
}

/// Decodes the preferences value shared by both answers.
pub(crate) fn decode_preferences(
    value: artisan_capnp::user_preferences::Reader<'_>,
) -> Result<UserPreferences, ProtocolDecodeError> {
    let default_engine_config = if value.has_default_engine_config() {
        Some(decode_engine_run_config(
            value.get_default_engine_config()?,
        )?)
    } else {
        None
    };
    let field = "userPreferences.projects";
    let encoded = value.get_projects()?;
    if encoded.len() as usize > NAVIGATION_PROJECTS_MAX {
        return Err(leaf::ComposerStateCodecError::StateValue { field }.into());
    }
    let projects = encoded
        .iter()
        .map(|entry| {
            Ok(NavigationProject {
                project_id: parse_project_id(read_text(entry.get_project_id(), field)?, field)?,
                last_thread_id: optional_thread(entry.get_last_thread_id(), field)?,
            })
        })
        .collect::<Result<Vec<_>, ProtocolDecodeError>>()?;
    let route = if value.has_route() {
        let route = value.get_route()?;
        let field = "userPreferences.route";
        Some(NavigationRoute {
            project_id: parse_project_id(read_text(route.get_project_id(), field)?, field)?,
            thread_id: optional_thread(route.get_thread_id(), field)?,
        })
    } else {
        None
    };
    let navigation = NavigationRecord::new(projects, route)
        .map_err(|_| leaf::ComposerStateCodecError::StateValue { field })?;
    let account = value.get_account()?;
    let name = |text, field: &'static str| {
        DisplayName::parse(read_text(text, field)?)
            .map_err(|source| ProtocolDecodeError::DisplayName { field, source })
    };
    Ok(UserPreferences {
        revision: value.get_revision(),
        default_engine_config,
        navigation,
        account: AccountProfile {
            display_name: name(
                account.get_display_name(),
                "userPreferences.account.displayName",
            )?,
            host_name: name(account.get_host_name(), "userPreferences.account.hostName")?,
        },
    })
}

fn optional_thread(
    value: capnp::Result<capnp::text::Reader<'_>>,
    field: &'static str,
) -> Result<Option<ThreadId>, ProtocolDecodeError> {
    let text = read_text(value, field)?;
    if text.is_empty() {
        Ok(None)
    } else {
        parse_thread_id(text, field).map(Some)
    }
}

const fn encode_outcome(value: LegacyImportOutcome) -> artisan_capnp::LegacyImportOutcome {
    match value {
        LegacyImportOutcome::Absent => artisan_capnp::LegacyImportOutcome::Absent,
        LegacyImportOutcome::Imported => artisan_capnp::LegacyImportOutcome::Imported,
        LegacyImportOutcome::Kept => artisan_capnp::LegacyImportOutcome::Kept,
        LegacyImportOutcome::Refused => artisan_capnp::LegacyImportOutcome::Refused,
    }
}

const fn decode_outcome(value: artisan_capnp::LegacyImportOutcome) -> LegacyImportOutcome {
    match value {
        artisan_capnp::LegacyImportOutcome::Absent => LegacyImportOutcome::Absent,
        artisan_capnp::LegacyImportOutcome::Imported => LegacyImportOutcome::Imported,
        artisan_capnp::LegacyImportOutcome::Kept => LegacyImportOutcome::Kept,
        artisan_capnp::LegacyImportOutcome::Refused => LegacyImportOutcome::Refused,
    }
}
