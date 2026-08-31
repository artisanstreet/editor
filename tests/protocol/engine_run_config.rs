//! Owned protocol coverage for the appended durable engine-configuration arms.

use std::error::Error;

use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, EngineAgentId, EngineConfigRevision,
    EngineConfigUpdatePrecondition, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, EngineVariantId, FilesystemAccess, FiniteMillis, NetworkAccess,
    OpenCode2Selection, PermissionId, ReceiptDisposition, RequestId, SetThreadEngineConfig,
    ThreadId, UnixMillis, WebSearchAccess,
};
use artisan_protocol::artisan_capnp::{envelope, request, response};
use artisan_protocol::{
    ClientRequest as ProtocolClientRequest, FrameId, ProtocolDecodeError, ProtocolEncodeError,
    ProtocolValueError, ResponsePayload, ServerResponse, WireEnvelope, WireEnvelopeBody,
    decode_envelope, encode_envelope,
};
use capnp::message::{Builder, HeapAllocator, ReaderOptions};
use capnp::serialize;

fn config(with_variant: bool) -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond is valid");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget is valid"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: ByteLimit::new(8_192).expect("json body limit is valid"),
        max_sse_line_bytes: ByteLimit::new(4_096).expect("sse line limit is valid"),
        max_sse_event_bytes: ByteLimit::new(8_192).expect("sse event limit is valid"),
        max_readiness_line_bytes: ByteLimit::new(4_096).expect("readiness line limit is valid"),
        max_header_count: CountLimit::new(8).expect("header count is valid"),
        max_http_buffer_bytes: ByteLimit::new(8_192).expect("http buffer limit is valid"),
        max_stderr_bytes: ByteLimit::new(4_096).expect("stderr limit is valid"),
        observation_capacity: CountLimit::new(16).expect("observation capacity is valid"),
    })
    .expect("runtime relationships are valid");
    let permission = EnginePermissionPolicy::new(
        PermissionId::parse("permission-protocol").expect("permission id is valid"),
        EngineAgentId::parse("agent-protocol").expect("agent id is valid"),
        ApprovalMode::OnRequest,
        FilesystemAccess::Workspace,
        NetworkAccess::Enabled,
        WebSearchAccess::Disabled,
    );
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile-protocol").expect("profile id is valid"),
            EngineModelId::parse("model-protocol").expect("model id is valid"),
            EngineRouteId::parse("route-protocol").expect("route id is valid"),
            with_variant
                .then(|| EngineVariantId::parse("variant-protocol").expect("variant id is valid")),
            permission,
        )),
        runtime,
    )
}

fn request(frame_id: &str, precondition: EngineConfigUpdatePrecondition) -> WireEnvelope {
    let request_id = RequestId::parse(frame_id).expect("request id is valid");
    WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(7),
        body: WireEnvelopeBody::Request(ProtocolClientRequest::Command(
            artisan_domain::Command::SetThreadEngineConfig(Box::new(SetThreadEngineConfig::new(
                request_id,
                ThreadId::parse("thread-protocol").expect("thread id is valid"),
                precondition,
                config(false),
            ))),
        )),
    }
}

fn raw_engine_config_frame(
    precondition_kind: &str,
    precondition_revision: u64,
    attempt: u64,
) -> Vec<u8> {
    let mut message = Builder::new(HeapAllocator::new());
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("raw-engine-config");
    root.set_sent_at_millis(7);
    let request = root.init_body().init_request();
    let mut encoded = request.init_set_thread_engine_config();
    encoded.set_thread_id("thread-protocol");
    let mut precondition = encoded.reborrow().init_precondition();
    precondition.set_kind(precondition_kind);
    precondition.set_revision(precondition_revision);
    let mut config = encoded.init_config();
    config.set_schema_version(1);
    config.set_engine("opencode2");
    config.set_profile_id("profile-protocol");
    config.set_model_id("model-protocol");
    config.set_route_id("route-protocol");
    let mut variant = config.reborrow().init_variant();
    variant.set_kind("none");
    variant.set_id("");
    let mut permission = config.reborrow().init_permission();
    permission.set_permission_id("permission-protocol");
    permission.set_agent_id("agent-protocol");
    permission.set_approval("on_request");
    permission.set_filesystem("workspace");
    permission.set_network("enabled");
    permission.set_web_search("disabled");
    let mut runtime = config.init_runtime();
    runtime.set_attempt_budget_ms(attempt);
    runtime.set_readiness_budget_ms(1);
    runtime.set_health_budget_ms(1);
    runtime.set_prompt_budget_ms(1);
    runtime.set_stream_budget_ms(1);
    runtime.set_close_budget_ms(1);
    runtime.set_max_json_body_bytes(8_192);
    runtime.set_max_sse_line_bytes(4_096);
    runtime.set_max_sse_event_bytes(8_192);
    runtime.set_max_readiness_line_bytes(4_096);
    runtime.set_max_header_count(8);
    runtime.set_max_http_buffer_bytes(8_192);
    runtime.set_max_stderr_bytes(4_096);
    runtime.set_observation_capacity(16);
    serialize::write_message_to_words(&message)
}

#[test]
fn request_and_response_round_trip_with_explicit_variant_states() -> Result<(), Box<dyn Error>> {
    for (with_variant, precondition) in [
        (false, EngineConfigUpdatePrecondition::Unconfigured),
        (
            true,
            EngineConfigUpdatePrecondition::Exact(
                EngineConfigRevision::new(4).expect("revision is valid"),
            ),
        ),
    ] {
        let mut value = request("engine-protocol-request", precondition);
        if with_variant
            && let WireEnvelopeBody::Request(ProtocolClientRequest::Command(
                artisan_domain::Command::SetThreadEngineConfig(command),
            )) = &mut value.body
        {
            **command = SetThreadEngineConfig::new(
                RequestId::parse("engine-protocol-request").expect("request id is valid"),
                ThreadId::parse("thread-protocol").expect("thread id is valid"),
                precondition,
                config(true),
            );
        }
        let encoded = encode_envelope(&value)?;
        assert!(decode_envelope(&encoded)? == value);
    }

    let response = WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse("server-engine-protocol").expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(8),
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("engine-protocol-response").expect("request id is valid"),
            payload: ResponsePayload::ThreadEngineConfigSet(
                artisan_protocol::SetThreadEngineConfigResult {
                    request_id: RequestId::parse("engine-protocol-response")
                        .expect("request id is valid"),
                    thread_id: ThreadId::parse("thread-protocol").expect("thread id is valid"),
                    revision: EngineConfigRevision::new(5).expect("revision is valid"),
                    disposition: ReceiptDisposition::Duplicate,
                },
            ),
        }),
    };
    let encoded = encode_envelope(&response)?;
    assert!(decode_envelope(&encoded)? == response);
    Ok(())
}

#[test]
fn malformed_engine_precondition_and_runtime_are_rejected_as_engine_errors() {
    let unsupported_precondition = decode_envelope(&raw_engine_config_frame("other", 0, 100));
    assert!(matches!(
        unsupported_precondition,
        Err(ProtocolDecodeError::EngineConfig { .. })
    ));

    let zero_attempt = decode_envelope(&raw_engine_config_frame("unconfigured", 0, 0));
    assert!(matches!(
        zero_attempt,
        Err(ProtocolDecodeError::EngineConfig { .. })
    ));
}

#[test]
fn appended_request_arm_is_visible_through_generated_union_bindings() -> capnp::Result<()> {
    let encoded = raw_engine_config_frame("unconfigured", 0, 100);
    let mut encoded = encoded.as_slice();
    let message = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())?;
    let root: envelope::Reader = message.get_root()?;
    let envelope::body::Which::Request(request) = root.get_body().which()? else {
        panic!("expected a request body");
    };
    assert!(matches!(
        request?.which()?,
        request::Which::SetThreadEngineConfig(_)
    ));
    Ok(())
}

#[test]
fn engine_config_error_code_is_appended_after_the_existing_codes() {
    assert_eq!(
        artisan_protocol::artisan_capnp::ErrorCode::EngineConfigConflict as u16,
        9
    );
}

#[test]
fn response_correlation_remains_fenced_for_the_appended_arm() {
    let value = WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse("server-engine-correlation").expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(8),
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("outer-request").expect("request id is valid"),
            payload: ResponsePayload::ThreadEngineConfigSet(
                artisan_protocol::SetThreadEngineConfigResult {
                    request_id: RequestId::parse("nested-request").expect("request id is valid"),
                    thread_id: ThreadId::parse("thread-protocol").expect("thread id is valid"),
                    revision: EngineConfigRevision::new(1).expect("revision is valid"),
                    disposition: ReceiptDisposition::Accepted,
                },
            ),
        }),
    };
    assert!(matches!(
        encode_envelope(&value),
        Err(artisan_protocol::ProtocolEncodeError::Value(
            ProtocolValueError::ResponseCorrelationMismatch
        ))
    ));
}

#[test]
fn appended_response_arm_is_visible_through_generated_union_bindings() -> capnp::Result<()> {
    let value = WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse("server-engine-arm").expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(8),
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("engine-arm-request").expect("request id is valid"),
            payload: ResponsePayload::ThreadEngineConfigSet(
                artisan_protocol::SetThreadEngineConfigResult {
                    request_id: RequestId::parse("engine-arm-request")
                        .expect("request id is valid"),
                    thread_id: ThreadId::parse("thread-protocol").expect("thread id is valid"),
                    revision: EngineConfigRevision::new(1).expect("revision is valid"),
                    disposition: ReceiptDisposition::Accepted,
                },
            ),
        }),
    };
    let encoded = encode_envelope(&value).expect("response should encode");
    let mut encoded = encoded.as_slice();
    let message = serialize::read_message_from_flat_slice(&mut encoded, ReaderOptions::new())?;
    let root: envelope::Reader = message.get_root()?;
    let envelope::body::Which::Response(response_value) = root.get_body().which()? else {
        panic!("expected a response body");
    };
    assert!(matches!(
        response_value?.which()?,
        response::Which::ThreadEngineConfigSet(_)
    ));
    Ok(())
}

fn read_settings_request_envelope(thread_id: &str, frame_id: &str) -> WireEnvelope {
    WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(11),
        body: WireEnvelopeBody::Request(ProtocolClientRequest::Query(
            artisan_domain::Query::ReadThreadEngineSettings(
                artisan_domain::commands::ReadThreadEngineSettings::new(
                    ThreadId::parse(thread_id).expect("thread id is valid"),
                ),
            ),
        )),
    }
}

fn read_settings_response_envelope(
    thread_id: &str,
    revision: Option<u64>,
    config: Option<EngineRunConfig>,
    frame_id: &str,
    request_id: &str,
) -> WireEnvelope {
    let payload = match (revision, config) {
        (Some(value), Some(value_config)) => ResponsePayload::ThreadEngineSettings(
            artisan_protocol::ThreadEngineSettingsResult::Configured {
                thread_id: ThreadId::parse(thread_id).expect("thread id is valid"),
                revision: EngineConfigRevision::new(value).expect("revision is valid"),
                config: Box::new(value_config),
            },
        ),
        _ => ResponsePayload::ThreadEngineSettings(
            artisan_protocol::ThreadEngineSettingsResult::Unconfigured {
                thread_id: ThreadId::parse(thread_id).expect("thread id is valid"),
            },
        ),
    };
    WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(12),
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse(request_id).expect("request id is valid"),
            payload,
        }),
    }
}

fn raw_read_settings_request_frame(thread_id: &str) -> Vec<u8> {
    let mut message = Builder::new(HeapAllocator::new());
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("raw-read-settings-request");
    root.set_sent_at_millis(11);
    root.init_body()
        .init_request()
        .init_read_thread_engine_settings()
        .set_thread_id(thread_id);
    serialize::write_message_to_words(&message)
}

fn raw_thread_engine_settings_response(
    thread_id: &str,
    state: &str,
    revision: u64,
    config_attempt: u64,
) -> Vec<u8> {
    let mut message = Builder::new(HeapAllocator::new());
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("raw-thread-engine-settings-response");
    root.set_sent_at_millis(12);
    let mut response = root.init_body().init_response();
    response.set_request_id("raw-read-settings-request");
    let mut settings = response.init_thread_engine_settings();
    settings.set_thread_id(thread_id);
    let mut state_builder = settings.init_state();
    match state {
        "unconfigured" => {
            state_builder.set_unconfigured(());
        }
        "configured" => {
            let mut configured = state_builder.init_configured();
            configured.set_revision(revision);
            let mut config = configured.init_config();
            config.set_schema_version(1);
            config.set_engine("opencode2");
            config.set_profile_id("profile-protocol");
            config.set_model_id("model-protocol");
            config.set_route_id("route-protocol");
            let mut variant = config.reborrow().init_variant();
            variant.set_kind("none");
            variant.set_id("");
            let mut permission = config.reborrow().init_permission();
            permission.set_permission_id("permission-protocol");
            permission.set_agent_id("agent-protocol");
            permission.set_approval("on_request");
            permission.set_filesystem("workspace");
            permission.set_network("enabled");
            permission.set_web_search("disabled");
            let mut runtime = config.init_runtime();
            runtime.set_attempt_budget_ms(config_attempt);
            runtime.set_readiness_budget_ms(1);
            runtime.set_health_budget_ms(1);
            runtime.set_prompt_budget_ms(1);
            runtime.set_stream_budget_ms(1);
            runtime.set_close_budget_ms(1);
            runtime.set_max_json_body_bytes(8_192);
            runtime.set_max_sse_line_bytes(4_096);
            runtime.set_max_sse_event_bytes(8_192);
            runtime.set_max_readiness_line_bytes(4_096);
            runtime.set_max_header_count(8);
            runtime.set_max_http_buffer_bytes(8_192);
            runtime.set_max_stderr_bytes(4_096);
            runtime.set_observation_capacity(16);
        }
        other => panic!("unknown state {other}"),
    }
    serialize::write_message_to_words(&message)
}

#[test]
fn read_thread_engine_settings_request_round_trip_carries_exact_thread_id_at_arm_12()
-> Result<(), Box<dyn Error>> {
    let value = read_settings_request_envelope("thread-protocol", "read-frame-1");
    let encoded = encode_envelope(&value)?;
    let decoded = decode_envelope(&encoded)?;
    assert!(decoded == value);
    let mut encoded_slice = encoded.as_slice();
    let message =
        serialize::read_message_from_flat_slice(&mut encoded_slice, ReaderOptions::new())?;
    let root: envelope::Reader = message.get_root()?;
    let envelope::body::Which::Request(req) = root.get_body().which()? else {
        panic!("expected request body");
    };
    assert!(matches!(
        req?.which()?,
        request::Which::ReadThreadEngineSettings(_)
    ));
    Ok(())
}

#[test]
fn thread_engine_settings_configured_response_round_trip_preserves_thread_revision_and_complete_config()
-> Result<(), Box<dyn Error>> {
    let value = read_settings_response_envelope(
        "thread-protocol",
        Some(7),
        Some(config(true)),
        "server-read-settings",
        "read-frame-1",
    );
    let encoded = encode_envelope(&value)?;
    let decoded = decode_envelope(&encoded)?;
    assert!(decoded == value);
    if let WireEnvelopeBody::Response(ServerResponse {
        payload: ResponsePayload::ThreadEngineSettings(result),
        ..
    }) = decoded.body
    {
        match result {
            artisan_protocol::ThreadEngineSettingsResult::Configured {
                thread_id,
                revision,
                config: actual_config,
            } => {
                assert_eq!(thread_id.as_str(), "thread-protocol");
                assert_eq!(revision.get(), 7);
                assert_eq!(*actual_config, config(true));
            }
            other @ artisan_protocol::ThreadEngineSettingsResult::Unconfigured { .. } => {
                panic!("expected configured result, got {other:?}")
            }
        }
    } else {
        panic!("expected thread engine settings response");
    }
    Ok(())
}

#[test]
fn thread_engine_settings_unconfigured_response_remains_distinct() -> Result<(), Box<dyn Error>> {
    let configured = read_settings_response_envelope(
        "thread-protocol",
        Some(3),
        Some(config(false)),
        "server-configured",
        "read-frame-1",
    );
    let unconfigured = read_settings_response_envelope(
        "thread-protocol",
        None,
        None,
        "server-unconfigured",
        "read-frame-2",
    );
    let encoded_configured = encode_envelope(&configured)?;
    let encoded_unconfigured = encode_envelope(&unconfigured)?;
    let decoded_configured = decode_envelope(&encoded_configured)?;
    let decoded_unconfigured = decode_envelope(&encoded_unconfigured)?;
    assert!(decoded_configured != decoded_unconfigured);
    assert!(matches!(
        decoded_unconfigured.body,
        WireEnvelopeBody::Response(ServerResponse {
            payload: ResponsePayload::ThreadEngineSettings(
                artisan_protocol::ThreadEngineSettingsResult::Unconfigured { .. },
            ),
            ..
        })
    ));
    // Ordinals: request @12, response @13 remain frozen
    let mut slice = encoded_configured.as_slice();
    let message = serialize::read_message_from_flat_slice(&mut slice, ReaderOptions::new())?;
    let root: envelope::Reader = message.get_root()?;
    let envelope::body::Which::Response(resp) = root.get_body().which()? else {
        panic!("expected response");
    };
    assert!(matches!(
        resp?.which()?,
        response::Which::ThreadEngineSettings(_)
    ));
    Ok(())
}

#[test]
fn thread_engine_settings_rejects_zero_configured_revision_invalid_id_malformed_config_and_trailing_bytes()
 {
    let zero_revision = decode_envelope(&raw_thread_engine_settings_response(
        "thread-protocol",
        "configured",
        0,
        100,
    ));
    assert!(matches!(
        zero_revision,
        Err(ProtocolDecodeError::EngineConfig { .. })
    ));

    let out_of_domain = decode_envelope(&raw_thread_engine_settings_response(
        "thread-protocol",
        "configured",
        i64::MAX as u64 + 1,
        100,
    ));
    assert!(matches!(
        out_of_domain,
        Err(ProtocolDecodeError::EngineConfig { .. })
    ));

    let invalid_thread = decode_envelope(&raw_read_settings_request_frame(""));
    assert!(matches!(
        invalid_thread,
        Err(ProtocolDecodeError::Identifier { .. })
    ));

    let malformed_config = decode_envelope(&raw_thread_engine_settings_response(
        "thread-protocol",
        "configured",
        1,
        0,
    ));
    assert!(matches!(
        malformed_config,
        Err(ProtocolDecodeError::EngineConfig { .. })
    ));

    let mut trailing = encode_envelope(&read_settings_request_envelope(
        "thread-protocol",
        "read-frame-trailing",
    ))
    .expect("request should encode");
    trailing.push(0xFF);
    let trailing_error = decode_envelope(&trailing);
    assert!(matches!(
        trailing_error,
        Err(ProtocolDecodeError::TrailingBytes { .. })
    ));
}

#[test]
fn thread_engine_settings_unknown_union_discriminant_is_rejected() {
    let valid = raw_thread_engine_settings_response("thread-protocol", "unconfigured", 1, 100);
    let configured = raw_thread_engine_settings_response("thread-protocol", "configured", 1, 100);
    let mut found = false;
    for index in 0..valid.len().min(configured.len()) {
        if valid[index] == configured[index] {
            continue;
        }
        let mut corrupted = valid.clone();
        corrupted[index] = 255;
        if matches!(
            decode_envelope(&corrupted),
            Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
        ) {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "no differing byte produced UnknownDiscriminant {{ value: 255 }}"
    );
}

fn list_registered_profiles_request_envelope(frame_id: &str) -> WireEnvelope {
    WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(13),
        body: WireEnvelopeBody::Request(ProtocolClientRequest::Query(
            artisan_domain::Query::ListRegisteredEngineProfiles(
                artisan_domain::commands::ListRegisteredEngineProfiles,
            ),
        )),
    }
}

fn registered_profiles_response_envelope(
    state: &str,
    ids: Vec<&str>,
    frame_id: &str,
    request_id: &str,
) -> WireEnvelope {
    let result = match state {
        "missing" => artisan_protocol::RegisteredEngineProfilesResult::RegistryMissing,
        "present" => artisan_protocol::RegisteredEngineProfilesResult::RegistryPresent {
            profile_ids: ids
                .into_iter()
                .map(|value| {
                    artisan_domain::EngineProfileId::parse(value).expect("profile id is valid")
                })
                .collect(),
        },
        other => panic!("unknown state {other}"),
    };
    WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(14),
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse(request_id).expect("request id is valid"),
            payload: ResponsePayload::RegisteredEngineProfiles(result),
        }),
    }
}

fn raw_registered_profiles_response(ids: &[&str], state: &str) -> Vec<u8> {
    let mut message = Builder::new(HeapAllocator::new());
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id("raw-registered-profiles-response");
    root.set_sent_at_millis(14);
    let mut response = root.init_body().init_response();
    response.set_request_id("raw-list-profiles");
    let result = response.init_registered_engine_profiles();
    match state {
        "missing" => {
            result.init_state().set_registry_missing(());
        }
        "present" => {
            let mut list = result
                .init_state()
                .init_registry_present()
                .init_profile_ids(
                    u32::try_from(ids.len()).expect("fixture profile list fits in u32"),
                );
            for (index, value) in ids.iter().enumerate() {
                list.set(
                    u32::try_from(index).expect("fixture index fits in u32"),
                    value,
                );
            }
        }
        other => panic!("unknown state {other}"),
    }
    serialize::write_message_to_words(&message)
}

#[test]
fn list_registered_profiles_request_round_trip_selects_appended_arm_13()
-> Result<(), Box<dyn Error>> {
    let value = list_registered_profiles_request_envelope("list-profiles-frame");
    let encoded = encode_envelope(&value)?;
    let decoded = decode_envelope(&encoded)?;
    assert!(decoded == value);
    let mut slice = encoded.as_slice();
    let message = serialize::read_message_from_flat_slice(&mut slice, ReaderOptions::new())?;
    let root: envelope::Reader = message.get_root()?;
    let envelope::body::Which::Request(req) = root.get_body().which()? else {
        panic!("expected request body");
    };
    assert!(matches!(
        req?.which()?,
        request::Which::ListRegisteredEngineProfiles(_)
    ));
    Ok(())
}

#[test]
fn registered_profiles_missing_present_empty_and_ordered_round_trip_through_arm_14_without_collapsing()
-> Result<(), Box<dyn Error>> {
    let missing = registered_profiles_response_envelope(
        "missing",
        vec![],
        "server-missing",
        "raw-list-profiles",
    );
    let present_empty = registered_profiles_response_envelope(
        "present",
        vec![],
        "server-present-empty",
        "raw-list-profiles",
    );
    let ordered = registered_profiles_response_envelope(
        "present",
        vec!["zeta", "alpha", "work.profile"],
        "server-ordered",
        "raw-list-profiles",
    );
    for value in [&missing, &present_empty, &ordered] {
        let encoded = encode_envelope(value)?;
        let decoded = decode_envelope(&encoded)?;
        assert!(decoded == *value);
    }
    assert!(missing != present_empty);
    assert!(present_empty != ordered);
    let encoded = encode_envelope(&ordered)?;
    let mut slice = encoded.as_slice();
    let message = serialize::read_message_from_flat_slice(&mut slice, ReaderOptions::new())?;
    let root: envelope::Reader = message.get_root()?;
    let envelope::body::Which::Response(resp) = root.get_body().which()? else {
        panic!("expected response body");
    };
    assert!(matches!(
        resp?.which()?,
        response::Which::RegisteredEngineProfiles(_)
    ));
    if let WireEnvelopeBody::Response(ServerResponse {
        payload:
            ResponsePayload::RegisteredEngineProfiles(
                artisan_protocol::RegisteredEngineProfilesResult::RegistryPresent { profile_ids },
            ),
        ..
    }) = decode_envelope(&encode_envelope(&ordered)?)?.body
    {
        assert_eq!(
            profile_ids
                .iter()
                .map(EngineProfileId::as_str)
                .collect::<Vec<_>>(),
            vec!["zeta", "alpha", "work.profile"]
        );
    } else {
        panic!("expected present ordered");
    }
    Ok(())
}

#[test]
fn registered_profiles_rejects_invalid_empty_duplicate_65th_malformed_unknown_and_trailing_bytes() {
    let empty_id = decode_envelope(&raw_registered_profiles_response(&[""], "present"));
    assert!(
        matches!(empty_id, Err(ProtocolDecodeError::EngineConfig { .. })),
        "empty profile id should be rejected"
    );

    let malformed = decode_envelope(&raw_registered_profiles_response(&["a/b"], "present"));
    assert!(
        matches!(malformed, Err(ProtocolDecodeError::EngineConfig { .. })),
        "malformed profile id should be rejected"
    );

    let duplicate = decode_envelope(&raw_registered_profiles_response(
        &["alpha", "alpha"],
        "present",
    ));
    assert!(
        matches!(duplicate, Err(ProtocolDecodeError::EngineConfig { .. })),
        "duplicate profile id should be rejected"
    );

    let distinct_many: Vec<String> = (0..65).map(|index| format!("profile-{index:02}")).collect();
    let distinct_many_refs: Vec<&str> = distinct_many.iter().map(String::as_str).collect();
    let over_limit = decode_envelope(&raw_registered_profiles_response(
        &distinct_many_refs,
        "present",
    ));
    assert!(
        matches!(over_limit, Err(ProtocolDecodeError::EngineConfig { .. })),
        "65th profile id should be rejected"
    );

    let valid = raw_registered_profiles_response(&[], "present");
    let missing = raw_registered_profiles_response(&[], "missing");
    let mut found_unknown = false;
    for index in 0..valid.len().min(missing.len()) {
        if valid[index] == missing[index] {
            continue;
        }
        let mut corrupted = valid.clone();
        corrupted[index] = 255;
        if matches!(
            decode_envelope(&corrupted),
            Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
        ) {
            found_unknown = true;
            break;
        }
    }
    assert!(
        found_unknown,
        "no differing byte produced UnknownDiscriminant for registered profiles"
    );

    let mut trailing = encode_envelope(&list_registered_profiles_request_envelope("list-trailing"))
        .expect("request should encode");
    trailing.push(0xFF);
    assert!(matches!(
        decode_envelope(&trailing),
        Err(ProtocolDecodeError::TrailingBytes { .. })
    ));

    let duplicate_envelope = registered_profiles_response_envelope(
        "present",
        vec!["alpha", "alpha"],
        "server-duplicate",
        "raw-list-profiles",
    );
    assert!(
        matches!(
            encode_envelope(&duplicate_envelope),
            Err(ProtocolEncodeError::Duplicate { .. })
        ),
        "duplicate present profile vector should be rejected on encode"
    );

    let distinct_over: Vec<String> = (0..65).map(|index| format!("profile-{index:02}")).collect();
    let distinct_over_refs: Vec<&str> = distinct_over.iter().map(String::as_str).collect();
    let over_limit_envelope = registered_profiles_response_envelope(
        "present",
        distinct_over_refs,
        "server-over-limit",
        "raw-list-profiles",
    );
    assert!(
        matches!(
            encode_envelope(&over_limit_envelope),
            Err(ProtocolEncodeError::CollectionTooLarge { .. })
        ),
        "65th profile should be rejected on encode"
    );
}

#[test]
fn pre1_engine_settings_ordinals_and_round_trips_remain_frozen() -> Result<(), Box<dyn Error>> {
    let read_request = read_settings_request_envelope("thread-protocol", "read-frame-pre1");
    let encoded = encode_envelope(&read_request)?;
    let mut slice = encoded.as_slice();
    let message = serialize::read_message_from_flat_slice(&mut slice, ReaderOptions::new())?;
    let root: envelope::Reader = message.get_root()?;
    let envelope::body::Which::Request(req) = root.get_body().which()? else {
        panic!("expected request");
    };
    assert!(matches!(
        req?.which()?,
        request::Which::ReadThreadEngineSettings(_)
    ));

    let configured = read_settings_response_envelope(
        "thread-protocol",
        Some(7),
        Some(config(true)),
        "server-pre1",
        "read-frame-pre1",
    );
    let encoded = encode_envelope(&configured)?;
    let mut slice = encoded.as_slice();
    let message = serialize::read_message_from_flat_slice(&mut slice, ReaderOptions::new())?;
    let root: envelope::Reader = message.get_root()?;
    let envelope::body::Which::Response(resp) = root.get_body().which()? else {
        panic!("expected response");
    };
    assert!(matches!(
        resp?.which()?,
        response::Which::ThreadEngineSettings(_)
    ));

    assert!(decode_envelope(&encode_envelope(&read_request)?)? == read_request);
    assert!(decode_envelope(&encode_envelope(&configured)?)? == configured);
    Ok(())
}
