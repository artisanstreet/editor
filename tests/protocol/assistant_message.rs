//! Owned assistant-item protocol coverage: production round trips through
//! `encode_envelope`/`decode_envelope`, typed raw-wire rejections for every
//! validated assistant field, and safe discriminant corruption proving the
//! appended arm stays finite.

use std::error::Error;

use artisan_domain::{
    AssistantBody, AssistantBodyError, AssistantMessageItem, AssistantMessagePhase,
    ConversationCursor, ConversationItem, ConversationLifecycle, ConversationPatch,
    ConversationSnapshot, ConversationSubscriptionStart, ConversationTurn, IdentifierError, ItemId,
    ItemOrdinal, MESSAGE_BODY_MAX_BYTES, MessageBody, PatchBatch, PatchId, PatchSequence,
    RequestId, Revision, RunId, ThreadId, TurnId, TurnOrdinal, UnixMillis, UserMessageItem,
};
use artisan_protocol::artisan_capnp::{
    AssistantMessagePhase as WirePhase, ConversationLifecycle as WireLifecycle,
    assistant_message_item, envelope,
};
use artisan_protocol::{
    ConversationSubscriptionStarted, FrameId, ProtocolDecodeError, ProtocolVersion,
    ResponsePayload, ServerResponse, WireEnvelope, WireEnvelopeBody, decode_envelope,
    encode_envelope,
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
