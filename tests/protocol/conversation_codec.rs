//! Owned conversation protocol conversion and malformed-input coverage.

use std::error::Error;

use artisan_domain::{
    CONVERSATION_PATCH_BATCH_MAX_PATCHES, CONVERSATION_QUERY_MAX_TURNS,
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES, ConversationCursor, ConversationItem,
    ConversationLifecycle, ConversationPatch, ConversationQuery, ConversationQueryBounds,
    ConversationRequest, ConversationSnapshot, ConversationSnapshotError, ConversationSubscribe,
    ConversationSubscriptionStart, ConversationTurn, ConversationUnsubscribe, CounterError,
    IncrementalText, IncrementalTextError, ItemId, ItemOrdinal, MessageBody, PatchBatch,
    PatchBatchError, PatchId, PatchSequence, QueryTurnCount, QueryTurnCountError, RequestId,
    Revision, ThreadId, TurnId, TurnOrdinal, UnixMillis, UserMessageItem,
};
use artisan_protocol::artisan_capnp::{ConversationLifecycle as WireLifecycle, envelope};
use artisan_protocol::{
    ClientRequest, ConversationSubscriptionStarted, ConversationSubscriptionStopped, FrameId,
    ProtocolDecodeError, ProtocolVersion, ResponsePayload, ServerResponse, WireEnvelope,
    WireEnvelopeBody, decode_envelope, encode_envelope,
};
use capnp::message::{Builder, HeapAllocator};
use capnp::serialize;

const THREAD_ID: &str = "thread-conversation-1";
const TURN_ID: &str = "turn-conversation-1";
const ITEM_ID: &str = "item-conversation-1";
const REQUEST_ID: &str = "request-conversation-1";

fn thread_id() -> ThreadId {
    ThreadId::parse(THREAD_ID).expect("fixture thread id is valid")
}

fn turn_id() -> TurnId {
    TurnId::parse(TURN_ID).expect("fixture turn id is valid")
}

fn item_id() -> ItemId {
    ItemId::parse(ITEM_ID).expect("fixture item id is valid")
}

fn turn(lifecycle: ConversationLifecycle) -> ConversationTurn {
    ConversationTurn {
        turn_id: turn_id(),
        ordinal: TurnOrdinal::new(0),
        revision: Revision::new(3),
        lifecycle,
        created_at: UnixMillis::from_millis(-10),
        updated_at: UnixMillis::from_millis(20),
    }
}

fn item(lifecycle: ConversationLifecycle) -> ConversationItem {
    ConversationItem::UserMessage(UserMessageItem {
        item_id: item_id(),
        turn_id: turn_id(),
        ordinal: ItemOrdinal::new(1),
        revision: Revision::new(2),
        lifecycle,
        body: MessageBody::parse("Queued conversation text").expect("fixture body is valid"),
        created_at: UnixMillis::from_millis(-5),
        updated_at: UnixMillis::from_millis(25),
    })
}

fn snapshot() -> ConversationSnapshot {
    ConversationSnapshot::new(
        thread_id(),
        ConversationCursor::new(7),
        vec![turn(ConversationLifecycle::Active)],
        vec![item(ConversationLifecycle::Completed)],
        UnixMillis::from_millis(30),
    )
    .expect("fixture snapshot is valid")
}

fn request_id() -> RequestId {
    RequestId::parse(REQUEST_ID).expect("fixture request id is valid")
}

fn envelope(frame_id: &str, body: WireEnvelopeBody) -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("fixture frame id is valid"),
        sent_at: UnixMillis::from_millis(40),
        body,
    }
}

fn assert_roundtrip(value: &WireEnvelope) -> Result<(), Box<dyn Error>> {
    let encoded = encode_envelope(value)?;
    let decoded = decode_envelope(&encoded)?;
    assert!(
        decoded == *value,
        "owned conversation frame changed in transit"
    );
    Ok(())
}

fn conversation_request(value: ConversationRequest) -> WireEnvelope {
    envelope(
        REQUEST_ID,
        WireEnvelopeBody::Request(ClientRequest::Conversation(value)),
    )
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

#[test]
fn every_conversation_request_roundtrips() -> Result<(), Box<dyn Error>> {
    let maximum = QueryTurnCount::new(u64::from(CONVERSATION_QUERY_MAX_TURNS))?;
    assert_roundtrip(&conversation_request(ConversationRequest::Query(
        ConversationQuery {
            thread_id: thread_id(),
            bounds: ConversationQueryBounds::Window {
                maximum_turn_count: maximum,
            },
        },
    )))?;
    for minimum_turn_ordinal in [None, Some(TurnOrdinal::new(2))] {
        assert_roundtrip(&conversation_request(ConversationRequest::Query(
            ConversationQuery {
                thread_id: thread_id(),
                bounds: ConversationQueryBounds::Range {
                    before_turn_ordinal: TurnOrdinal::new(12),
                    minimum_turn_ordinal,
                    maximum_turn_count: maximum,
                },
            },
        )))?;
    }
    assert_roundtrip(&conversation_request(ConversationRequest::Subscribe(
        ConversationSubscribe::fresh(thread_id()),
    )))?;
    for cursor in [0, 9] {
        assert_roundtrip(&conversation_request(ConversationRequest::Subscribe(
            ConversationSubscribe::resume(thread_id(), ConversationCursor::new(cursor)),
        )))?;
    }
    assert_roundtrip(&conversation_request(ConversationRequest::Unsubscribe(
        ConversationUnsubscribe {
            thread_id: thread_id(),
        },
    )))
}

#[test]
fn every_conversation_response_roundtrips() -> Result<(), Box<dyn Error>> {
    assert_roundtrip(&response(
        "server-conversation-snapshot",
        ResponsePayload::ConversationSnapshot(snapshot()),
    ))?;
    assert_roundtrip(&response(
        "server-conversation-started-fresh",
        ResponsePayload::ConversationSubscriptionStarted(ConversationSubscriptionStarted::Fresh(
            ConversationSubscriptionStart::new(snapshot()),
        )),
    ))?;
    assert_roundtrip(&response(
        "server-conversation-started-resumed",
        ResponsePayload::ConversationSubscriptionStarted(
            ConversationSubscriptionStarted::Resumed {
                thread_id: thread_id(),
                cursor: ConversationCursor::new(7),
            },
        ),
    ))?;
    assert_roundtrip(&response(
        "server-conversation-stopped",
        ResponsePayload::ConversationSubscriptionStopped(ConversationSubscriptionStopped {
            thread_id: thread_id(),
        }),
    ))
}

fn patch_id(value: &str) -> PatchId {
    PatchId::parse(value).expect("fixture patch id is valid")
}

fn patch_sequence(value: u64) -> PatchSequence {
    PatchSequence::new(value).expect("fixture sequence is positive")
}

fn all_patch_variants() -> Vec<ConversationPatch> {
    vec![
        ConversationPatch::TurnUpsert {
            patch_id: patch_id("patch-turn-upsert"),
            sequence: patch_sequence(1),
            turn: turn(ConversationLifecycle::Active),
        },
        ConversationPatch::ItemUpsert {
            patch_id: patch_id("patch-item-upsert"),
            sequence: patch_sequence(2),
            item: item(ConversationLifecycle::Completed),
        },
        ConversationPatch::ItemAppend {
            patch_id: patch_id("patch-item-append"),
            sequence: patch_sequence(3),
            item_id: item_id(),
            revision: Revision::new(3),
            text: IncrementalText::parse(" delta").expect("fixture fragment is valid"),
        },
        ConversationPatch::ItemLifecycle {
            patch_id: patch_id("patch-item-lifecycle"),
            sequence: patch_sequence(4),
            item_id: item_id(),
            revision: Revision::new(4),
            lifecycle: ConversationLifecycle::Failed,
        },
        ConversationPatch::TurnLifecycle {
            patch_id: patch_id("patch-turn-lifecycle"),
            sequence: patch_sequence(5),
            turn_id: turn_id(),
            revision: Revision::new(4),
            lifecycle: ConversationLifecycle::Cancelled,
        },
    ]
}

#[test]
fn every_patch_variant_roundtrips_in_one_contiguous_batch() -> Result<(), Box<dyn Error>> {
    let batch = PatchBatch::new(
        thread_id(),
        ConversationCursor::default(),
        ConversationCursor::new(5),
        all_patch_variants(),
    )?;
    assert_roundtrip(&envelope(
        "server-conversation-patches",
        WireEnvelopeBody::PatchBatch(batch),
    ))
}

#[test]
fn every_conversation_lifecycle_roundtrips() -> Result<(), Box<dyn Error>> {
    let lifecycles = [
        ConversationLifecycle::Pending,
        ConversationLifecycle::Streaming,
        ConversationLifecycle::Active,
        ConversationLifecycle::Waiting,
        ConversationLifecycle::Completed,
        ConversationLifecycle::Failed,
        ConversationLifecycle::Interrupted,
        ConversationLifecycle::Cancelled,
    ];
    for (index, lifecycle) in lifecycles.into_iter().enumerate() {
        let patch = ConversationPatch::TurnLifecycle {
            patch_id: patch_id(&format!("patch-lifecycle-{index}")),
            sequence: patch_sequence(1),
            turn_id: turn_id(),
            revision: Revision::new(
                u64::try_from(index).expect("fixture lifecycle index fits u64"),
            ),
            lifecycle,
        };
        let batch = PatchBatch::new(
            thread_id(),
            ConversationCursor::default(),
            ConversationCursor::new(1),
            vec![patch],
        )?;
        assert_roundtrip(&envelope(
            &format!("server-lifecycle-{index}"),
            WireEnvelopeBody::PatchBatch(batch),
        ))?;
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
    root.set_sent_at_millis(40);
    root
}

fn words(message: &Builder<HeapAllocator>) -> Vec<u8> {
    serialize::write_message_to_words(message)
}

fn decode_error(bytes: &[u8]) -> ProtocolDecodeError {
    let Err(error) = decode_envelope(bytes) else {
        panic!("malformed conversation frame must be rejected");
    };
    error
}

#[test]
fn malformed_query_counts_return_typed_errors() {
    for (value, expected) in [
        (0, QueryTurnCountError::Zero),
        (
            CONVERSATION_QUERY_MAX_TURNS + 1,
            QueryTurnCountError::TooLarge {
                value: u64::from(CONVERSATION_QUERY_MAX_TURNS + 1),
                maximum: CONVERSATION_QUERY_MAX_TURNS,
            },
        ),
    ] {
        let mut message = raw_message();
        let mut query = init_raw_envelope(&mut message, REQUEST_ID)
            .init_body()
            .init_request()
            .init_conversation_query();
        query.set_thread_id(THREAD_ID);
        query
            .init_bounds()
            .init_window()
            .set_maximum_turn_count(value);
        let error = decode_error(&words(&message));
        let ProtocolDecodeError::QueryTurnCount { source } = error else {
            panic!("query bound must return QueryTurnCount error");
        };
        assert_eq!(source, expected);
    }
}

#[test]
fn oversized_snapshot_and_patch_batch_return_typed_errors_before_elements() {
    let oversized_snapshot = {
        let mut message = raw_message();
        let mut response = init_raw_envelope(&mut message, "server-oversized-snapshot")
            .init_body()
            .init_response();
        response.set_request_id(REQUEST_ID);
        let mut snapshot = response.init_conversation_snapshot();
        snapshot.set_thread_id(THREAD_ID);
        snapshot
            .reborrow()
            .init_turns(u32::from(CONVERSATION_QUERY_MAX_TURNS) + 1);
        snapshot.init_items(0);
        words(&message)
    };
    let ProtocolDecodeError::ConversationSnapshot { source } = decode_error(&oversized_snapshot)
    else {
        panic!("oversized snapshot must return ConversationSnapshot error");
    };
    assert_eq!(
        source,
        ConversationSnapshotError::TooManyTurns {
            count: usize::from(CONVERSATION_QUERY_MAX_TURNS) + 1,
            maximum: usize::from(CONVERSATION_QUERY_MAX_TURNS),
        }
    );

    let oversized_batch = {
        let mut message = raw_message();
        let mut batch = init_raw_envelope(&mut message, "server-oversized-batch")
            .init_body()
            .init_patch_batch();
        batch.set_thread_id(THREAD_ID);
        batch.init_patches(
            u32::try_from(CONVERSATION_PATCH_BATCH_MAX_PATCHES + 1)
                .expect("fixture count fits u32"),
        );
        words(&message)
    };
    let ProtocolDecodeError::PatchBatch { source } = decode_error(&oversized_batch) else {
        panic!("oversized batch must return PatchBatch error");
    };
    assert_eq!(
        source,
        PatchBatchError::TooManyPatches {
            count: CONVERSATION_PATCH_BATCH_MAX_PATCHES + 1,
            maximum: CONVERSATION_PATCH_BATCH_MAX_PATCHES,
        }
    );
}

fn raw_append_batch(sequence: u64, text: &str) -> Vec<u8> {
    let mut message = raw_message();
    let mut batch = init_raw_envelope(&mut message, "server-append-batch")
        .init_body()
        .init_patch_batch();
    batch.set_thread_id(THREAD_ID);
    batch.set_from_cursor(0);
    batch.set_to_cursor(sequence);
    let mut patches = batch.init_patches(1);
    let mut patch = patches.reborrow().get(0);
    patch.set_patch_id("patch-append");
    patch.set_sequence(sequence);
    let mut append = patch.init_item_append();
    append.set_item_id(ITEM_ID);
    append.set_revision(1);
    append.set_text(text);
    words(&message)
}

#[test]
fn zero_patch_sequence_and_oversized_fragment_return_typed_errors() {
    let error = decode_error(&raw_append_batch(0, ""));
    assert!(matches!(
        error,
        ProtocolDecodeError::Counter {
            source: CounterError::ZeroPatchSequence,
            ..
        }
    ));

    let fragment = "x".repeat(CONVERSATION_TEXT_FRAGMENT_MAX_BYTES + 1);
    let error = decode_error(&raw_append_batch(1, &fragment));
    assert!(matches!(
        error,
        ProtocolDecodeError::IncrementalText {
            source: IncrementalTextError::TooLong {
                length,
                maximum: CONVERSATION_TEXT_FRAGMENT_MAX_BYTES,
            }
        } if length == CONVERSATION_TEXT_FRAGMENT_MAX_BYTES + 1
    ));
}

fn set_raw_turn(
    mut value: artisan_protocol::artisan_capnp::conversation_turn::Builder<'_>,
    id: &str,
    ordinal: u64,
) {
    value.set_turn_id(id);
    value.set_ordinal(ordinal);
    value.set_lifecycle(WireLifecycle::Pending);
}

#[test]
fn malformed_snapshot_structure_and_reserved_item_are_typed() {
    let duplicate_turn = {
        let mut message = raw_message();
        let mut response = init_raw_envelope(&mut message, "server-duplicate-turn")
            .init_body()
            .init_response();
        response.set_request_id(REQUEST_ID);
        let mut snapshot = response.init_conversation_snapshot();
        snapshot.set_thread_id(THREAD_ID);
        let mut turns = snapshot.reborrow().init_turns(2);
        set_raw_turn(turns.reborrow().get(0), TURN_ID, 0);
        set_raw_turn(turns.get(1), TURN_ID, 1);
        snapshot.init_items(0);
        words(&message)
    };
    let ProtocolDecodeError::ConversationSnapshot { source } = decode_error(&duplicate_turn) else {
        panic!("duplicate turn must return ConversationSnapshot error");
    };
    assert_eq!(
        source,
        ConversationSnapshotError::DuplicateTurnId { turn_id: turn_id() }
    );

    let unknown_turn = {
        let mut message = raw_message();
        let mut response = init_raw_envelope(&mut message, "server-unknown-turn")
            .init_body()
            .init_response();
        response.set_request_id(REQUEST_ID);
        let mut snapshot = response.init_conversation_snapshot();
        snapshot.set_thread_id(THREAD_ID);
        let turns = snapshot.reborrow().init_turns(1);
        set_raw_turn(turns.get(0), TURN_ID, 0);
        let items = snapshot.reborrow().init_items(1);
        let mut item = items.get(0).init_user_message();
        item.set_item_id(ITEM_ID);
        item.set_turn_id("turn-missing");
        item.set_ordinal(1);
        item.set_lifecycle(WireLifecycle::Pending);
        item.set_body("Queued text");
        words(&message)
    };
    let ProtocolDecodeError::ConversationSnapshot { source } = decode_error(&unknown_turn) else {
        panic!("unknown turn must return ConversationSnapshot error");
    };
    assert_eq!(
        source,
        ConversationSnapshotError::UnknownTurn {
            item_id: item_id(),
            turn_id: TurnId::parse("turn-missing").expect("fixture turn id is valid"),
        }
    );

    let reserved_item = {
        let mut message = raw_message();
        let mut batch = init_raw_envelope(&mut message, "server-reserved-item")
            .init_body()
            .init_patch_batch();
        batch.set_thread_id(THREAD_ID);
        batch.set_to_cursor(1);
        let mut patches = batch.init_patches(1);
        let mut patch = patches.reborrow().get(0);
        patch.set_patch_id("patch-reserved-item");
        patch.set_sequence(1);
        patch.init_item_upsert().set_unmodeled(());
        words(&message)
    };
    assert!(matches!(
        decode_error(&reserved_item),
        ProtocolDecodeError::UnmodeledConversationItem
    ));
}

fn raw_lifecycle_batch(lifecycle: WireLifecycle) -> Vec<u8> {
    let mut message = raw_message();
    let mut batch = init_raw_envelope(&mut message, "server-lifecycle")
        .init_body()
        .init_patch_batch();
    batch.set_thread_id(THREAD_ID);
    batch.set_to_cursor(1);
    let mut patches = batch.init_patches(1);
    let mut patch = patches.reborrow().get(0);
    patch.set_patch_id("patch-lifecycle");
    patch.set_sequence(1);
    let mut transition = patch.init_turn_lifecycle();
    transition.set_turn_id(TURN_ID);
    transition.set_lifecycle(lifecycle);
    words(&message)
}

#[test]
fn unknown_conversation_lifecycle_discriminant_is_typed() {
    let mut malformed = raw_lifecycle_batch(WireLifecycle::Pending);
    let comparison = raw_lifecycle_batch(WireLifecycle::Streaming);
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
