//! Parent-union dispatch for the business decisions the Forge sends as data
//! (stateless Editor step 6): the scope-free host catalog with the Forge's
//! account readiness applied, and the resolution of a model selection into
//! the engine configuration the Forge would run.

#[allow(clippy::wildcard_imports)]
use super::*;

use artisan_domain::{ModelSelectionResolution, ResolveModelSelection};

use crate::composer_state_codec as leaf;

/// Encodes one Forge-decision request arm.
pub(crate) fn encode_forge_decision_request(
    mut builder: request::Builder<'_>,
    value: &ClientRequest,
) -> Result<(), ProtocolEncodeError> {
    match value {
        ClientRequest::Query(Query::ReadHostCatalog(_)) => {
            builder.set_read_host_catalog(());
            Ok(())
        }
        ClientRequest::Query(Query::ResolveModelSelection(query)) => {
            let mut encoded = builder.init_resolve_model_selection();
            encoded.set_thread_id(query.thread_id.as_str());
            leaf::encode_catalog_selection(encoded.init_selection(), &query.selection);
            Ok(())
        }
        _ => Err(ProtocolEncodeError::ComposerState),
    }
}

/// Decodes one Forge-decision request arm.
pub(crate) fn decode_forge_decision_request(
    value: request::Reader<'_>,
) -> Result<ClientRequest, ProtocolDecodeError> {
    match value.which()? {
        request::Which::ReadHostCatalog(()) => Ok(ClientRequest::Query(Query::ReadHostCatalog(
            ReadHostCatalog,
        ))),
        request::Which::ResolveModelSelection(query) => {
            let query = query?;
            let field = "request.resolveModelSelection.threadId";
            Ok(ClientRequest::Query(Query::ResolveModelSelection(
                ResolveModelSelection {
                    thread_id: parse_thread_id(read_text(query.get_thread_id(), field)?, field)?,
                    selection: leaf::decode_catalog_selection(
                        query.get_selection()?,
                        "request.resolveModelSelection.selection",
                    )?,
                },
            )))
        }
        _ => Err(
            crate::composer_state_codec::ComposerStateCodecError::StateValue {
                field: "request.forgeDecision",
            }
            .into(),
        ),
    }
}

/// Encodes one Forge-decision response arm.
pub(crate) fn encode_forge_decision_response(
    mut builder: response::Builder<'_>,
    payload: &ResponsePayload,
) -> Result<(), ProtocolEncodeError> {
    match payload {
        ResponsePayload::HostCatalog(snapshot) => {
            builder
                .reborrow()
                .init_host_catalog()
                .set_snapshot_data(snapshot.as_bytes());
            Ok(())
        }
        ResponsePayload::ModelSelectionResolved(resolution) => {
            let mut encoded = builder.reborrow().init_model_selection_resolved();
            encoded.set_thread_id(resolution.thread_id.as_str());
            leaf::encode_catalog_selection(
                encoded.reborrow().init_selection(),
                &resolution.selection,
            );
            match &resolution.outcome {
                Ok(config) => encode_engine_run_config(encoded.init_resolved(), config),
                Err(refusal) => leaf::encode_submission_refusal(encoded.init_refused(), refusal),
            }
            Ok(())
        }
        _ => Err(ProtocolEncodeError::ComposerState),
    }
}

/// Decodes one Forge-decision response arm.
pub(crate) fn decode_forge_decision_response(
    value: response::Reader<'_>,
) -> Result<ResponsePayload, ProtocolDecodeError> {
    match value.which()? {
        response::Which::HostCatalog(result) => Ok(ResponsePayload::HostCatalog(
            CatalogSnapshotWire::new(result?.get_snapshot_data()?.to_vec())?,
        )),
        response::Which::ModelSelectionResolved(resolution) => {
            let resolution = resolution?;
            let field = "response.modelSelectionResolved.threadId";
            let outcome = match resolution.which()? {
                artisan_capnp::model_selection_resolution::Which::Resolved(config) => {
                    Ok(decode_engine_run_config(config?)?)
                }
                artisan_capnp::model_selection_resolution::Which::Refused(refusal) => {
                    Err(leaf::decode_submission_refusal(
                        refusal?,
                        "response.modelSelectionResolved.refused",
                    )?)
                }
            };
            Ok(ResponsePayload::ModelSelectionResolved(
                ModelSelectionResolution {
                    thread_id: parse_thread_id(
                        read_text(resolution.get_thread_id(), field)?,
                        field,
                    )?,
                    selection: leaf::decode_catalog_selection(
                        resolution.get_selection()?,
                        "response.modelSelectionResolved.selection",
                    )?,
                    outcome,
                },
            ))
        }
        _ => Err(
            crate::composer_state_codec::ComposerStateCodecError::StateValue {
                field: "response.forgeDecision",
            }
            .into(),
        ),
    }
}
