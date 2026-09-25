//! Parent-union dispatch for the business decisions the Forge sends as data
//! (stateless Editor step 6): the scope-free host catalog with the Forge's
//! account readiness applied.

#[allow(clippy::wildcard_imports)]
use super::*;

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
        _ => Err(
            crate::composer_state_codec::ComposerStateCodecError::StateValue {
                field: "response.forgeDecision",
            }
            .into(),
        ),
    }
}
