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
    ClientRequest as ProtocolClientRequest, FrameId, ProtocolDecodeError, ProtocolValueError,
    ResponsePayload, ServerResponse, WireEnvelope, WireEnvelopeBody, decode_envelope,
    encode_envelope,
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
    let mut request = root.init_body().init_request();
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
        if let WireEnvelopeBody::Request(ProtocolClientRequest::Command(
            artisan_domain::Command::SetThreadEngineConfig(command),
        )) = &mut value.body
        {
            if with_variant {
                *command = Box::new(SetThreadEngineConfig::new(
                    RequestId::parse("engine-protocol-request").expect("request id is valid"),
                    ThreadId::parse("thread-protocol").expect("thread id is valid"),
                    precondition,
                    config(true),
                ));
            }
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
