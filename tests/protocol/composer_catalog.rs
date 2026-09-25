//! Focused owned-protocol coverage for the runtime composer catalog and
//! durable model-favorites packets.
//!
//! The catalog fixture comes from the shared typed catalog wire API. These
//! tests intentionally do not launch an engine or fabricate a provider turn.

use std::error::Error;

use artisan_catalog::NativeCatalogScope;
use artisan_catalog::NativeModelCatalog;
use artisan_catalog::wire::encode_catalog;
use artisan_domain::{
    CATALOG_REVISION_MAX_BYTES, CatalogRevision, CatalogRevisionError, Command, EngineProfileId,
    ModelFavoriteId, ModelFavoritesRevision, ModelFavoritesSnapshotError, Query,
    ReadComposerCatalog, ReadHostCatalog, ReadModelFavorites, ReceiptDisposition, RequestId,
    SetModelFavorite, ThreadId, UnixMillis,
};
use artisan_protocol::artisan_capnp::{ReceiptDisposition as WireDisposition, envelope};
use artisan_protocol::{
    CATALOG_SNAPSHOT_MAX_BYTES, CatalogSnapshotWire, CatalogSnapshotWireError, ClientRequest,
    ComposerCatalogResult, FrameId, ModelFavoritesSnapshot, ProtocolDecodeError,
    ProtocolEncodeError, ProtocolValueError, ResponsePayload, ServerResponse, WireEnvelope,
    WireEnvelopeBody, decode_envelope, encode_envelope,
};
use capnp::message::{Builder, HeapAllocator};
use capnp::serialize;

fn thread_id() -> ThreadId {
    ThreadId::parse("thread-catalog-protocol").expect("thread id is valid")
}

fn profile_id() -> EngineProfileId {
    EngineProfileId::parse("profile-catalog-protocol").expect("profile id is valid")
}

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("request id is valid")
}

fn model_id(value: &str) -> ModelFavoriteId {
    ModelFavoriteId::parse(value).expect("model id is valid")
}

fn revision(value: &str) -> CatalogRevision {
    CatalogRevision::parse(value).expect("catalog revision is valid")
}

fn envelope_for(frame_id: &str, body: WireEnvelopeBody) -> WireEnvelope {
    WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(42),
        body,
    }
}

fn response_for(request_name: &str, payload: ResponsePayload) -> WireEnvelope {
    envelope_for(
        "server-composer-catalog-frame",
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id(request_name),
            payload,
        }),
    )
}

fn scoped_catalog_with_long_native_id() -> NativeModelCatalog {
    let mut catalog =
        NativeModelCatalog::from_manifest_json(include_str!("../fixtures/model_catalog.json"))
            .expect("fixture catalog is valid");

    // The fixture manifest is immutable in the shared wire format. Append a
    // dynamic non-OpenCode row so a native identifier longer than the legacy
    // 128-byte identifier rule is exercised without changing its prefix.
    let mut dynamic = catalog
        .manifest
        .models
        .iter()
        .find(|model| model.native_selection.is_none())
        .cloned()
        .expect("fixture catalog has a model without a native selection");
    dynamic.id = "dynamic-long-native-model-".to_owned() + &"x".repeat(128);
    "Dynamic long native model".clone_into(&mut dynamic.name);
    "native-model".clone_into(&mut dynamic.native_model_id);
    "dynamic".clone_into(&mut dynamic.status);
    assert!(dynamic.id.len() > 128);
    catalog.manifest.models.push(dynamic);

    let mut runtime = catalog.runtime();
    runtime.scope = Some(NativeCatalogScope {
        profile_id: profile_id().as_str().to_owned(),
        working_directory: "C:/workspace/composer".to_owned(),
        workspace_trust: "trusted_project_config".to_owned(),
    });
    catalog.replace_runtime(runtime);
    catalog
}

fn favorites_snapshot() -> ModelFavoritesSnapshot {
    ModelFavoritesSnapshot::new(
        ModelFavoritesRevision::new(7).expect("revision is valid"),
        vec![model_id("model-codex"), model_id("model-opencode")],
    )
    .expect("favorites snapshot is valid")
}

fn raw_mismatched_receipt_frame() -> Vec<u8> {
    let mut message = Builder::new(HeapAllocator::new());
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("server-favorite-mismatch");
    root.set_sent_at_millis(42);

    let mut response = root.init_body().init_response();
    response.set_request_id("outer-favorite-request");
    let mut receipt = response.init_model_favorite_set();
    receipt.set_request_id("inner-favorite-request");
    receipt.set_model_id("model-opencode");
    receipt.set_favorite(true);
    receipt.set_disposition(WireDisposition::Accepted);
    let mut snapshot = receipt.init_snapshot();
    snapshot.set_revision(1);
    snapshot.init_model_ids(1).set(0, "model-opencode");

    serialize::write_message_to_words(&message)
}

fn raw_oversized_favorites_frame(count: usize) -> Vec<u8> {
    let mut message = Builder::new(HeapAllocator::new());
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("server-favorites-too-many");
    root.set_sent_at_millis(42);

    let mut response = root.init_body().init_response();
    response.set_request_id("favorites-too-many-request");
    let mut snapshot = response.init_model_favorites();
    snapshot.set_revision(0);
    let mut ids = snapshot.init_model_ids(u32::try_from(count).expect("fixture count fits"));
    for index in 0..count {
        let value = format!("model-{index}");
        ids.set(
            u32::try_from(index).expect("fixture index fits"),
            value.as_str(),
        );
    }

    serialize::write_message_to_words(&message)
}

#[test]
fn domain_catalog_revision_and_favorite_command_are_bounded_and_correlated() {
    assert_eq!(CatalogRevision::parse(""), Err(CatalogRevisionError::Empty));
    assert!(matches!(
        CatalogRevision::parse("x\n"),
        Err(CatalogRevisionError::ControlCharacter { character: '\n' })
    ));
    assert!(matches!(
        CatalogRevision::parse("x".repeat(CATALOG_REVISION_MAX_BYTES + 1)),
        Err(CatalogRevisionError::TooLong { .. })
    ));

    let request = request_id("set-favorite-domain-request");
    let command = SetModelFavorite::new(
        request.clone(),
        thread_id(),
        profile_id(),
        revision("catalog-revision-7"),
        model_id("model-opencode"),
        true,
    );
    assert_eq!(command.request_id(), &request);
    assert_eq!(Command::SetModelFavorite(command).request_id(), &request);
}

#[test]
fn catalog_snapshot_preserves_scope_and_long_native_identity() -> Result<(), Box<dyn Error>> {
    let catalog = scoped_catalog_with_long_native_id();
    let bytes = encode_catalog(&catalog)?;
    let wire = CatalogSnapshotWire::new(bytes.clone())?;
    assert_eq!(wire.as_bytes(), bytes.as_slice());
    assert_eq!(wire.decoded()?, catalog);
    assert!(format!("{wire:?}").contains("bytes_len"));
    assert!(!format!("{wire:?}").contains("native-model-"));

    let result = ComposerCatalogResult::new(thread_id(), profile_id(), wire)?;
    let value = response_for(
        "catalog-response-request",
        ResponsePayload::ComposerCatalog(result),
    );
    assert!(decode_envelope(&encode_envelope(&value)?)? == value);
    Ok(())
}

#[test]
fn catalog_result_rejects_missing_or_mismatched_runtime_scope() -> Result<(), Box<dyn Error>> {
    let catalog =
        NativeModelCatalog::from_manifest_json(include_str!("../fixtures/model_catalog.json"))
            .expect("fixture catalog is valid");
    let wire = CatalogSnapshotWire::new(encode_catalog(&catalog)?)?;
    assert_eq!(
        ComposerCatalogResult::new(thread_id(), profile_id(), wire),
        Err(ProtocolValueError::CatalogScopeMissing)
    );

    let mut scoped = scoped_catalog_with_long_native_id();
    let mut runtime = scoped.runtime();
    runtime.scope.as_mut().expect("scope exists").profile_id = "other-profile".to_owned();
    scoped.replace_runtime(runtime);
    let wire = CatalogSnapshotWire::new(encode_catalog(&scoped)?)?;
    assert_eq!(
        ComposerCatalogResult::new(thread_id(), profile_id(), wire),
        Err(ProtocolValueError::CatalogScopeMismatch)
    );
    Ok(())
}

#[test]
fn catalog_snapshot_rejects_invalid_and_oversized_bytes_before_decode() {
    assert!(matches!(
        CatalogSnapshotWire::new(vec![0; CATALOG_SNAPSHOT_MAX_BYTES + 1]),
        Err(CatalogSnapshotWireError::TooLarge { .. })
    ));
    assert!(matches!(
        CatalogSnapshotWire::new(vec![b'{', b'}']),
        Err(CatalogSnapshotWireError::InvalidCatalog { .. })
    ));
}

#[test]
fn appended_requests_round_trip_through_owned_codec() -> Result<(), Box<dyn Error>> {
    let favorite_request_id = request_id("set-favorite-request");
    let requests = vec![
        envelope_for(
            "read-catalog-request",
            WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadComposerCatalog(
                ReadComposerCatalog::new(thread_id(), profile_id()),
            ))),
        ),
        envelope_for(
            "read-favorites-request",
            WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadModelFavorites(
                ReadModelFavorites,
            ))),
        ),
        envelope_for(
            favorite_request_id.as_str(),
            WireEnvelopeBody::Request(ClientRequest::Command(Command::SetModelFavorite(
                SetModelFavorite::new(
                    favorite_request_id.clone(),
                    thread_id(),
                    profile_id(),
                    revision("catalog-revision-7"),
                    model_id("model-opencode"),
                    false,
                ),
            ))),
        ),
    ];

    for request in requests {
        assert!(decode_envelope(&encode_envelope(&request)?)? == request);
    }
    Ok(())
}

#[test]
fn favorites_snapshot_and_receipt_round_trip_without_losing_order() -> Result<(), Box<dyn Error>> {
    let snapshot = favorites_snapshot();
    let snapshot_response = response_for(
        "favorites-response-request",
        ResponsePayload::ModelFavorites(snapshot.clone()),
    );
    assert!(decode_envelope(&encode_envelope(&snapshot_response)?)? == snapshot_response);

    let request_id = request_id("set-favorite-response-request");
    let receipt = artisan_protocol::SetModelFavoriteReceipt {
        request_id: request_id.clone(),
        model_id: model_id("model-opencode"),
        favorite: true,
        disposition: ReceiptDisposition::Accepted,
        snapshot,
    };
    let receipt_response = envelope_for(
        "server-favorite-receipt-frame",
        WireEnvelopeBody::Response(ServerResponse {
            request_id,
            payload: ResponsePayload::ModelFavoriteSet(receipt),
        }),
    );
    assert!(decode_envelope(&encode_envelope(&receipt_response)?)? == receipt_response);
    Ok(())
}

#[test]
fn favorite_receipt_correlation_is_checked_on_encode_and_decode() {
    let receipt = artisan_protocol::SetModelFavoriteReceipt {
        request_id: request_id("inner-favorite-request"),
        model_id: model_id("model-opencode"),
        favorite: true,
        disposition: ReceiptDisposition::Duplicate,
        snapshot: favorites_snapshot(),
    };
    let value = response_for(
        "outer-favorite-request",
        ResponsePayload::ModelFavoriteSet(receipt),
    );
    assert!(matches!(
        encode_envelope(&value),
        Err(ProtocolEncodeError::Value(
            ProtocolValueError::ResponseCorrelationMismatch
        ))
    ));

    assert!(matches!(
        decode_envelope(&raw_mismatched_receipt_frame()),
        Err(ProtocolDecodeError::CorrelationMismatch {
            field: "response.modelFavoriteSet.requestId"
        })
    ));
}

#[test]
fn favorite_decoder_checks_list_length_before_allocating_ids() {
    let count = artisan_domain::MODEL_FAVORITES_MAX_MODELS + 1;
    assert!(matches!(
        decode_envelope(&raw_oversized_favorites_frame(count)),
        Err(ProtocolDecodeError::ModelFavoritesSnapshot {
            source: ModelFavoritesSnapshotError::TooManyModels {
                count: actual,
                ..
            }
        }) if actual == count
    ));
}

#[test]
fn host_catalog_request_and_snapshot_round_trip() -> Result<(), Box<dyn Error>> {
    let request = envelope_for(
        "host-catalog-request",
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadHostCatalog(
            ReadHostCatalog,
        ))),
    );
    assert!(decode_envelope(&encode_envelope(&request)?)? == request);

    // The scope-free catalog carries no thread scope.
    let mut catalog =
        NativeModelCatalog::from_manifest_json(include_str!("../fixtures/model_catalog.json"))?;
    catalog.runnable_harness_ids = vec!["codex".to_owned()];
    let snapshot = CatalogSnapshotWire::new(encode_catalog(&catalog)?)?;
    let response = response_for(
        "host-catalog",
        ResponsePayload::HostCatalog(snapshot.clone()),
    );
    let decoded = decode_envelope(&encode_envelope(&response)?)?;
    assert!(decoded == response);
    let WireEnvelopeBody::Response(ServerResponse {
        payload: ResponsePayload::HostCatalog(decoded),
        ..
    }) = decoded.body
    else {
        panic!("expected hostCatalog payload");
    };
    assert_eq!(decoded, snapshot);
    assert_eq!(
        decoded.decoded()?.runnable_harness_ids,
        vec!["codex".to_owned()]
    );
    Ok(())
}
