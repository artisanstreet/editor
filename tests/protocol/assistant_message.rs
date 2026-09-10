//! Owned assistant-item protocol coverage: production round trips through
//! `encode_envelope`/`decode_envelope`, typed raw-wire rejections for every
//! validated assistant field, and safe discriminant corruption proving the
//! appended arm stays finite.

use std::error::Error;

use artisan_domain::{
    AuthoredText, AssistantBody, AssistantBodyError, AssistantMessageItem, AssistantMessagePhase,
    Command, ConversationCursor, ConversationItem, ConversationLifecycle, ConversationPatch,
    ConversationSnapshot, ConversationSubscriptionStart, ConversationTurn, EngineId,
    IdentifierError, ImageAttachment, ImageAttachmentRef, ItemId, ItemOrdinal,
    MESSAGE_BODY_MAX_BYTES, MessageBody, MessageId, PatchBatch, PatchId, PatchSequence,
    QueueMessage, QueueMessagePayload, RequestId, Revision, RunId, ThreadId, TurnId, TurnOrdinal,
    UnixMillis, UserMessageItem,
};
use artisan_protocol::artisan_capnp::{
    AssistantMessagePhase as WirePhase, ConversationLifecycle as WireLifecycle,
    RunStatus as WireRunStatus, active_run_result, assistant_message_item, envelope,
};
use artisan_protocol::{
    ActiveRunResult, ClientRequest, ConversationSubscriptionStarted, FrameId, ProtocolDecodeError,
    ProtocolVersion, ResponsePayload, RunLiveStatus, ServerResponse, WireEnvelope, WireEnvelopeBody,
    decode_envelope, encode_envelope,
};
use capnp::message::{Builder, HeapAllocator};
use capnp::serialize;

const THREAD_ID: &str = "thread-assist-proto-1";
const TURN_ID: &str = "turn-assist-proto-1";
const ITEM_ID: &str = "item-assist-proto-1";
const RUN_ID: &str = "run-assist-proto-1";
const REQUEST_ID: &str = "request-assist-proto-1";
const ASSISTANT_BODY: &str = "Settled assistant reply";
const USER_BODY: &str = "Queued question";

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD_ID).expect("fixture thread id is valid")
}

fn turn_id() -> TurnId {
    TurnId::parse(TURN_ID).expect("fixture turn id is valid")
}

fn run_id() -> RunId {
    RunId::parse(RUN_ID).expect("fixture run id is valid")
}

fn item_id(value: &str) -> ItemId {
    ItemId::parse(value).expect("fixture item id is valid")
}

fn request_id() -> RequestId {
    RequestId::parse(REQUEST_ID).expect("fixture request id is valid")
}

fn turn(lifecycle: ConversationLifecycle) -> ConversationTurn {
    ConversationTurn {
        turn_id: turn_id(),
        ordinal: TurnOrdinal::new(0),
        revision: Revision::new(1),
        lifecycle,
        created_at: UnixMillis::from_millis(-11),
        updated_at: UnixMillis::from_millis(21),
    }
}

fn user_item() -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: item_id("item-user-proto-1"),
        turn_id: turn_id(),
        ordinal: ItemOrdinal::new(1),
        revision: Revision::new(2),
        lifecycle: ConversationLifecycle::Completed,
        body: MessageBody::parse(USER_BODY).expect("fixture body is valid"),
        created_at: UnixMillis::from_millis(-5),
        updated_at: UnixMillis::from_millis(25),
    })
}

fn image_reference(message: &str, index: u32) -> ImageAttachmentRef {
    ImageAttachmentRef::new(
        MessageId::parse(message).expect("fixture message id is valid"),
        thread_id(),
        index,
        "image/png",
        format!("capture-{index}.png"),
        3,
        [index as u8; 32],
    )
    .expect("fixture image reference is valid")
}

fn multimodal_snapshot() -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(10),
        vec![turn(ConversationLifecycle::Completed)],
        vec![
            ConversationItem::MultimodalUserMessage(
                artisan_domain::MultimodalUserMessageItem {
                    item_id: item_id("item-mixed-proto-1"),
                    turn_id: turn_id(),
                    ordinal: ItemOrdinal::new(1),
                    revision: Revision::new(0),
                    lifecycle: ConversationLifecycle::Completed,
                    text: Some(AuthoredText::parse("caption").expect("caption is valid")),
                    attachments: vec![image_reference("message-mixed-proto-1", 0)],
                    created_at: UnixMillis::from_millis(1),
                    updated_at: UnixMillis::from_millis(2),
                },
            ),
            ConversationItem::MultimodalUserMessage(
                artisan_domain::MultimodalUserMessageItem {
                    item_id: item_id("item-image-only-proto-1"),
                    turn_id: turn_id(),
                    ordinal: ItemOrdinal::new(2),
                    revision: Revision::new(0),
                    lifecycle: ConversationLifecycle::Completed,
                    text: None,
                    attachments: vec![
                        image_reference("message-image-only-proto-1", 0),
                        image_reference("message-image-only-proto-1", 1),
                    ],
                    created_at: UnixMillis::from_millis(3),
                    updated_at: UnixMillis::from_millis(4),
                },
            ),
        ],
        UnixMillis::from_millis(5),
    )
    .expect("fixture multimodal snapshot is valid")
}

fn assistant_item(
    phase: AssistantMessagePhase,
    lifecycle: ConversationLifecycle,
) -> ConversationItem {
    ConversationItem::AssistantMessage(AssistantMessageItem {
        item_id: item_id(ITEM_ID),
        turn_id: turn_id(),
        run_id: run_id(),
        ordinal: ItemOrdinal::new(2),
        revision: Revision::new(4),
        lifecycle,
        body: AssistantBody::parse(ASSISTANT_BODY).expect("fixture body is valid"),
        phase,
        created_at: UnixMillis::from_millis(-7),
        updated_at: UnixMillis::from_millis(33),
    })
}

fn mixed_snapshot() -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(9),
        vec![turn(ConversationLifecycle::Active)],
        vec![
            user_item(),
            assistant_item(
                AssistantMessagePhase::Final,
                ConversationLifecycle::Completed,
            ),
        ],
        UnixMillis::from_millis(40),
    )
    .expect("fixture snapshot is valid")
}

fn envelope(frame_id: &str, body: WireEnvelopeBody) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("fixture frame id is valid"),
        sent_at: UnixMillis::from_millis(44),
        body,
    }
}

fn response(frame_id: &str, payload: ResponsePayload) -> WireEnvelope {
    envelope(
        frame_id,
        WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id(),
            payload,
        }),
    )
}

fn queue_request(frame_id: &str, payload: QueueMessagePayload) -> WireEnvelope {
    let request_id = RequestId::parse(frame_id).expect("queue frame id is a valid request id");
    envelope(
        frame_id,
        WireEnvelopeBody::Request(ClientRequest::Command(Command::QueueMessage(
            QueueMessage::new(request_id, thread_id(), payload),
        ))),
    )
}

fn assert_roundtrip(value: &WireEnvelope) -> Result<(), Box<dyn Error>> {
    let encoded = encode_envelope(value)?;
    let decoded = decode_envelope(&encoded)?;
    assert!(
        decoded == *value,
        "owned assistant frame changed in transit"
    );
    Ok(())
}

#[test]
fn mixed_assistant_snapshot_response_roundtrips_through_production_codec()
-> Result<(), Box<dyn Error>> {
    let value = response(
        "server-assist-snapshot",
        ResponsePayload::ConversationSnapshot(mixed_snapshot()),
    );
    let encoded = encode_envelope(&value)?;
    let decoded = decode_envelope(&encoded)?;
    assert!(decoded == value, "owned assistant frame changed in transit");

    // Field-inspect the actual production decode result; the full-value
    // equality above alone stays silent about which arm carried each kind.
    let WireEnvelopeBody::Response(decoded_response) = &decoded.body else {
        panic!("the decoded frame must remain a response");
    };
    let ResponsePayload::ConversationSnapshot(snapshot) = &decoded_response.payload else {
        panic!("the decoded response must carry a conversation snapshot");
    };
    let items = snapshot.items();
    assert_eq!(items.len(), 2);
    assert!(matches!(&items[0], ConversationItem::UserMessage(_)));
    let ConversationItem::AssistantMessage(message) = &items[1] else {
        panic!("the assistant kind must survive the production codec");
    };
    assert_eq!(message.item_id.as_str(), ITEM_ID);
    assert_eq!(message.turn_id.as_str(), TURN_ID);
    assert_eq!(message.run_id.as_str(), RUN_ID);
    assert_eq!(message.ordinal.get(), 2);
    assert_eq!(message.revision.get(), 4);
    assert_eq!(message.lifecycle, ConversationLifecycle::Completed);
    assert_eq!(message.body.as_str(), ASSISTANT_BODY);
    assert_eq!(message.phase, AssistantMessagePhase::Final);
    Ok(())
}

#[test]
fn mixed_and_image_only_items_roundtrip_without_image_bytes_in_history()
-> Result<(), Box<dyn Error>> {
    let value = response(
        "server-multimodal-snapshot",
        ResponsePayload::ConversationSnapshot(multimodal_snapshot()),
    );
    let decoded = decode_envelope(&encode_envelope(&value)?)?;
    assert!(decoded == value, "wire envelope round-trip mismatch");

    let WireEnvelopeBody::Response(response) = decoded.body else {
        panic!("multimodal frame must remain a response");
    };
    let ResponsePayload::ConversationSnapshot(snapshot) = response.payload else {
        panic!("multimodal frame must remain a snapshot");
    };
    let [ConversationItem::MultimodalUserMessage(mixed),
        ConversationItem::MultimodalUserMessage(image_only)] = snapshot.items() else {
        panic!("both multimodal item arms must survive the codec");
    };
    assert_eq!(mixed.text.as_ref().map(AuthoredText::as_str), Some("caption"));
    assert_eq!(mixed.attachments[0].size_bytes, 3);
    assert_eq!(image_only.text, None);
    assert_eq!(image_only.attachments.len(), 2);
    assert_eq!(image_only.attachments[1].index, 1);
    Ok(())
}

#[test]
fn mixed_and_image_only_queue_payloads_roundtrip_with_ordered_bytes()
-> Result<(), Box<dyn Error>> {
    let mixed = QueueMessagePayload::new(
        Some(AuthoredText::parse("caption").expect("caption is valid")),
        vec![
            ImageAttachment::new("image/png", vec![1, 2, 3], "first.png")
                .expect("first image is valid"),
            ImageAttachment::new("image/jpeg", vec![4, 5], "second.jpg")
                .expect("second image is valid"),
        ],
    )
    .expect("mixed payload is valid");
    let image_only = QueueMessagePayload::new(
        None,
        vec![ImageAttachment::new("image/webp", vec![6, 7], "only.webp")
            .expect("image-only attachment is valid")],
    )
    .expect("image-only payload is valid");

    for (frame_id, payload) in [
        ("request-mixed-payload", mixed),
        ("request-image-only-payload", image_only),
    ] {
        let value = queue_request(frame_id, payload);
        let decoded = decode_envelope(&encode_envelope(&value)?)?;
        assert!(decoded == value, "wire envelope round-trip mismatch");
        let WireEnvelopeBody::Request(ClientRequest::Command(Command::QueueMessage(command))) =
            decoded.body
        else {
            panic!("queue payload must remain a QueueMessage command");
        };
        assert_eq!(command.thread_id, thread_id());
        assert_eq!(command.payload.attachments().len(), if frame_id.contains("mixed") {
            2
        } else {
            1
        });
    }
    Ok(())
}

#[test]
fn fresh_subscription_started_roundtrips_with_assistant_items() -> Result<(), Box<dyn Error>> {
    assert_roundtrip(&response(
        "server-assist-started",
        ResponsePayload::ConversationSubscriptionStarted(ConversationSubscriptionStarted::Fresh(
            ConversationSubscriptionStart::new(mixed_snapshot()),
        )),
    ))
}

#[test]
fn assistant_item_upsert_patch_roundtrips_in_one_batch() -> Result<(), Box<dyn Error>> {
    let batch = PatchBatch::new(
        thread_id(),
        ConversationCursor::default(),
        ConversationCursor::new(1),
        vec![ConversationPatch::ItemUpsert {
            patch_id: PatchId::parse("patch-assist-upsert").expect("fixture patch id is valid"),
            sequence: PatchSequence::new(1).expect("fixture sequence is positive"),
            item: assistant_item(
                AssistantMessagePhase::Unspecified,
                ConversationLifecycle::Streaming,
            ),
        }],
    )?;
    assert_roundtrip(&envelope(
        "server-assist-patch",
        WireEnvelopeBody::PatchBatch(batch),
    ))
}

#[test]
fn every_phase_roundtrips_through_production_codec_independent_of_lifecycle()
-> Result<(), Box<dyn Error>> {
    // Positive production-codec conversions for every renderer phase, each
    // paired with a different lifecycle so neither field can mask the
    // other; Final deliberately rides a Pending lifecycle.
    for (phase, lifecycle) in [
        (
            AssistantMessagePhase::Unspecified,
            ConversationLifecycle::Streaming,
        ),
        (
            AssistantMessagePhase::Commentary,
            ConversationLifecycle::Active,
        ),
        (AssistantMessagePhase::Final, ConversationLifecycle::Pending),
    ] {
        let value = envelope(
            "server-assist-phase-walk",
            WireEnvelopeBody::PatchBatch(PatchBatch::new(
                thread_id(),
                ConversationCursor::default(),
                ConversationCursor::new(1),
                vec![ConversationPatch::ItemUpsert {
                    patch_id: PatchId::parse("patch-assist-phase")
                        .expect("fixture patch id is valid"),
                    sequence: PatchSequence::new(1).expect("fixture sequence is positive"),
                    item: assistant_item(phase, lifecycle),
                }],
            )?),
        );
        let decoded = decode_envelope(&encode_envelope(&value)?)?;
        assert!(decoded == value, "owned assistant frame changed in transit");

        let WireEnvelopeBody::PatchBatch(decoded_batch) = &decoded.body else {
            panic!("the decoded frame must remain a patch batch");
        };
        let [patch] = decoded_batch.patches() else {
            panic!("the fixture carries exactly one patch");
        };
        let ConversationPatch::ItemUpsert {
            item: ConversationItem::AssistantMessage(message),
            ..
        } = patch
        else {
            panic!("the upsert must stay an assistant item");
        };
        assert_eq!(message.phase, phase);
        assert_eq!(message.lifecycle, lifecycle);
        assert_eq!(message.run_id.as_str(), RUN_ID);
    }
    Ok(())
}

#[test]
fn empty_and_whitespace_assistant_bodies_roundtrip_through_production_codec()
-> Result<(), Box<dyn Error>> {
    for body_text in ["", "   \t\n "] {
        let value = envelope(
            "server-assist-open-body",
            WireEnvelopeBody::PatchBatch(PatchBatch::new(
                thread_id(),
                ConversationCursor::default(),
                ConversationCursor::new(1),
                vec![ConversationPatch::ItemUpsert {
                    patch_id: PatchId::parse("patch-assist-body")
                        .expect("fixture patch id is valid"),
                    sequence: PatchSequence::new(1).expect("fixture sequence is positive"),
                    item: ConversationItem::AssistantMessage(AssistantMessageItem {
                        item_id: item_id(ITEM_ID),
                        turn_id: turn_id(),
                        run_id: run_id(),
                        ordinal: ItemOrdinal::new(2),
                        revision: Revision::new(4),
                        lifecycle: ConversationLifecycle::Pending,
                        body: AssistantBody::parse(body_text)
                            .expect("opening assistant bodies stay valid"),
                        phase: AssistantMessagePhase::Commentary,
                        created_at: UnixMillis::from_millis(-7),
                        updated_at: UnixMillis::from_millis(33),
                    }),
                }],
            )?),
        );
        let decoded = decode_envelope(&encode_envelope(&value)?)?;
        assert!(decoded == value, "owned assistant frame changed in transit");

        let WireEnvelopeBody::PatchBatch(decoded_batch) = &decoded.body else {
            panic!("the decoded frame must remain a patch batch");
        };
        let [patch] = decoded_batch.patches() else {
            panic!("the fixture carries exactly one patch");
        };
        let ConversationPatch::ItemUpsert {
            item: ConversationItem::AssistantMessage(message),
            ..
        } = patch
        else {
            panic!("the upsert must stay an assistant item");
        };
        // The wire preserves opening bodies byte for byte.
        assert_eq!(message.body.as_str(), body_text);
        assert_eq!(message.phase, AssistantMessagePhase::Commentary);
    }
    Ok(())
}

fn raw_message() -> Builder<HeapAllocator> {
    Builder::new(HeapAllocator::new())
}

fn init_raw_envelope<'a>(
    message: &'a mut Builder<HeapAllocator>,
    frame_id: &'a str,
) -> envelope::Builder<'a> {
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id(frame_id);
    root.set_sent_at_millis(44);
    root
}

fn words(message: &Builder<HeapAllocator>) -> Vec<u8> {
    serialize::write_message_to_words(message)
}

fn decode_error(bytes: &[u8]) -> ProtocolDecodeError {
    let Err(error) = decode_envelope(bytes) else {
        panic!("malformed assistant frame must be rejected");
    };
    error
}

/// Builds one otherwise-valid raw snapshot response carrying exactly one
/// assistant item stamped by the caller.
fn raw_assistant_snapshot(
    customize: impl FnOnce(&mut assistant_message_item::Builder<'_>),
) -> Vec<u8> {
    let mut message = raw_message();
    let mut response = init_raw_envelope(&mut message, "server-assist-raw")
        .init_body()
        .init_response();
    response.set_request_id(REQUEST_ID);
    let mut snapshot = response.init_conversation_snapshot();
    snapshot.set_thread_id(THREAD_ID);
    snapshot.set_cursor(3);
    {
        let turns = snapshot.reborrow().init_turns(1);
        let mut raw_turn = turns.get(0);
        raw_turn.set_turn_id(TURN_ID);
        raw_turn.set_ordinal(0);
        raw_turn.set_lifecycle(WireLifecycle::Pending);
    }
    {
        let items = snapshot.reborrow().init_items(1);
        let mut item = items.get(0).init_assistant_message();
        item.set_item_id(ITEM_ID);
        item.set_turn_id(TURN_ID);
        item.set_run_id(RUN_ID);
        item.set_ordinal(2);
        item.set_revision(4);
        item.set_lifecycle(WireLifecycle::Completed);
        item.set_body(ASSISTANT_BODY);
        item.set_created_at_millis(-7);
        item.set_updated_at_millis(33);
        item.set_phase(WirePhase::Final);
        customize(&mut item);
    }
    snapshot.set_updated_at_millis(40);
    words(&message)
}

#[test]
fn raw_invalid_run_item_and_turn_ids_return_typed_errors() {
    let error = decode_error(&raw_assistant_snapshot(|item| {
        item.set_run_id("run leaked id");
    }));
    let ProtocolDecodeError::Identifier { field, source } = error else {
        panic!("invalid run id must return an Identifier error");
    };
    assert_eq!(field, "conversationItem.assistantMessage.runId");
    assert_eq!(
        source,
        IdentifierError::ForbiddenCharacter { character: ' ' }
    );

    let error = decode_error(&raw_assistant_snapshot(|item| {
        item.set_item_id("");
    }));
    let ProtocolDecodeError::Identifier { field, source } = error else {
        panic!("invalid item id must return an Identifier error");
    };
    assert_eq!(field, "conversationItem.assistantMessage.itemId");
    assert_eq!(source, IdentifierError::Empty);

    let error = decode_error(&raw_assistant_snapshot(|item| {
        item.set_turn_id("turn\tid");
    }));
    let ProtocolDecodeError::Identifier { field, source } = error else {
        panic!("invalid turn id must return an Identifier error");
    };
    assert_eq!(field, "conversationItem.assistantMessage.turnId");
    assert_eq!(
        source,
        IdentifierError::ForbiddenCharacter { character: '\t' }
    );
}

#[test]
fn raw_overlong_assistant_body_returns_the_typed_bound_error() {
    let oversized = "x".repeat(MESSAGE_BODY_MAX_BYTES + 1);
    let error = decode_error(&raw_assistant_snapshot(|item| {
        item.set_body(&oversized);
    }));
    let ProtocolDecodeError::AssistantBody { source } = error else {
        panic!("an over-long assistant body must return an AssistantBody error");
    };
    assert_eq!(
        source,
        AssistantBodyError::TooLong {
            length: MESSAGE_BODY_MAX_BYTES + 1,
            maximum: MESSAGE_BODY_MAX_BYTES,
        }
    );
}

#[test]
fn reserved_unmodeled_arm_stays_typedly_rejected() {
    let reserved = {
        let mut message = raw_message();
        let mut batch = init_raw_envelope(&mut message, "server-assist-reserved")
            .init_body()
            .init_patch_batch();
        batch.set_thread_id(THREAD_ID);
        batch.set_from_cursor(0);
        batch.set_to_cursor(1);
        let mut patches = batch.init_patches(1);
        let mut patch = patches.reborrow().get(0);
        patch.set_patch_id("patch-assist-reserved");
        patch.set_sequence(1);
        patch.init_item_upsert().set_unmodeled(());
        words(&message)
    };
    assert!(matches!(
        decode_error(&reserved),
        ProtocolDecodeError::UnmodeledConversationItem
    ));
}

#[test]
fn unknown_phase_discriminant_returns_unknown_discriminant() {
    let baseline = raw_assistant_snapshot(|_| ());
    assert!(
        decode_envelope(&baseline).is_ok(),
        "the untouched raw assistant fixture must stay conforming"
    );

    let mut malformed = raw_assistant_snapshot(|item| {
        item.set_phase(WirePhase::Commentary);
    });
    let comparison = baseline;
    let differing: Vec<usize> = malformed
        .iter()
        .zip(comparison)
        .enumerate()
        .filter_map(|(index, (left, right))| (left != &right).then_some(index))
        .collect();
    assert_eq!(differing.len(), 1, "only the phase ordinal should differ");
    malformed[differing[0]] = u8::MAX;
    assert!(matches!(
        decode_error(&malformed),
        ProtocolDecodeError::UnknownDiscriminant { value: 255 }
    ));
}

#[test]
fn unknown_lifecycle_discriminant_returns_unknown_discriminant() {
    let mut malformed = raw_assistant_snapshot_with_lifecycle(WireLifecycle::Streaming);
    let comparison = raw_assistant_snapshot_with_lifecycle(WireLifecycle::Pending);
    let differing: Vec<usize> = malformed
        .iter()
        .zip(comparison)
        .enumerate()
        .filter_map(|(index, (left, right))| (left != &right).then_some(index))
        .collect();
    assert_eq!(
        differing.len(),
        1,
        "only the lifecycle ordinal should differ"
    );
    malformed[differing[0]] = u8::MAX;
    assert!(matches!(
        decode_error(&malformed),
        ProtocolDecodeError::UnknownDiscriminant { value: 255 }
    ));
}

fn raw_assistant_snapshot_with_lifecycle(lifecycle: WireLifecycle) -> Vec<u8> {
    raw_assistant_snapshot(|item| {
        item.set_lifecycle(lifecycle);
    })
}

/// Builds one otherwise-valid raw active-run response, letting the caller
/// override the status and engine fields after the valid defaults.
fn raw_active_run(
    customize: impl FnOnce(&mut active_run_result::Builder<'_>),
) -> Vec<u8> {
    let mut message = raw_message();
    let mut response = init_raw_envelope(&mut message, "server-active-raw")
        .init_body()
        .init_response();
    response.set_request_id(REQUEST_ID);
    {
        let mut active = response.init_active_run();
        active.set_thread_id(THREAD_ID);
        active.init_state().set_active(RUN_ID);
        active.set_run_status(WireRunStatus::Running);
        active.set_run_engine_id("codex");
        customize(&mut active);
    }
    words(&message)
}

#[test]
fn active_run_status_and_engine_roundtrip_through_production_codec(
) -> Result<(), Box<dyn Error>> {
    let value = response(
        "server-active-roundtrip",
        ResponsePayload::ActiveRun(ActiveRunResult::Active {
            thread_id: thread_id(),
            run_id: run_id(),
            status: RunLiveStatus::Running,
            engine_id: EngineId::Codex,
        }),
    );
    let decoded = decode_envelope(&encode_envelope(&value)?)?;
    assert_eq!(decoded, value, "active run status must survive the wire");
    let WireEnvelopeBody::Response(decoded_response) = decoded.body else {
        panic!("decoded frame must remain a response");
    };
    let ResponsePayload::ActiveRun(ActiveRunResult::Active {
        status,
        engine_id,
        ..
    }) = decoded_response.payload
    else {
        panic!("decoded response must carry an active run");
    };
    assert_eq!(status, RunLiveStatus::Running);
    assert_eq!(engine_id, EngineId::Codex);
    Ok(())
}

#[test]
fn raw_unknown_run_status_returns_the_typed_strict_error() {
    let error = decode_error(&raw_active_run(|active| {
        active.set_run_status(WireRunStatus::Unknown);
    }));
    assert!(
        matches!(
            error,
            ProtocolDecodeError::UnknownDiscriminant { value: 0 }
        ),
        "unknown run status must fail closed, got {error:?}"
    );
}

#[test]
fn raw_empty_run_engine_returns_a_typed_error() {
    let error = decode_error(&raw_active_run(|active| {
        active.set_run_engine_id("");
    }));
    assert!(
        matches!(error, ProtocolDecodeError::EngineConfig { .. }),
        "empty run engine must fail closed, got {error:?}"
    );
}

#[test]
fn raw_invalid_steer_run_id_returns_a_typed_identifier_error() {
    let mut message = raw_message();
    let mut request = init_raw_envelope(&mut message, "client-steer-raw")
        .init_body()
        .init_request();
    {
        let mut queue = request.init_queue_message();
        queue.set_thread_id(THREAD_ID);
        queue.init_text().set_present("steer me");
        queue.init_attachments(0);
        queue.set_steer_run_id("run leaked id");
    }
    let error = decode_error(&words(&message));
    let ProtocolDecodeError::Identifier { field, source } = error else {
        panic!("invalid steer run id must return an Identifier error, got {error:?}");
    };
    assert_eq!(field, "request.queueMessage.steerRunId");
    assert_eq!(
        source,
        IdentifierError::ForbiddenCharacter { character: ' ' }
    );
}
